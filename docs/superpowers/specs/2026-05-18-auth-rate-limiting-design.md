# Auth Rate Limiting — Design Spec

**Date:** 2026-05-18  
**Phase:** 5  
**Status:** Approved, pending implementation plan  
**Addresses:** "No rate limiting on auth endpoints" — High severity debt from Phase 4 handoff

---

## Problem

`/auth/login`, `/auth/register`, and `/auth/refresh` are unprotected against credential-stuffing
and brute-force attacks. Any IP can hammer them without consequence.

---

## Scope

- Rate limit the three auth endpoints per remote IP using in-memory token buckets
- Return `429 Too Many Requests` when a bucket is exhausted
- Wire integration tests that assert the 429 path
- No Redis, no trusted-proxy header parsing, no structured error bodies (separate debt items)

---

## Crate

**`tower_governor`** with `PeerIpKeyExtractor`.

> **Version note:** The Phase 4 handoff cited `"0.4"`, but axum 0.8 uses tower 0.5. The first
> implementation step is to verify on crates.io which `tower_governor` release depends on tower 0.5
> and use that version. If 0.4 is incompatible, use the latest 0.5+ release.

`tower` moves from dev-dependency to a regular dependency in `signalnode-api/Cargo.toml`.

---

## Rate Limits

| Route | Tokens / min | `per_second(N)` | `burst_size` | Rationale |
|---|---|---|---|---|
| `POST /auth/register` | 5 | `per_second(12)` | 5 | Account creation is rarely burst-legitimate |
| `POST /auth/login` | 10 | `per_second(6)` | 10 | Allows a few fast retries before throttling |
| `POST /auth/refresh` | 30 | `per_second(2)` | 30 | Background token rotation is more frequent |

All keyed by remote IP via `PeerIpKeyExtractor` (reads `ConnectInfo<SocketAddr>`).

---

## Architecture

### Where the layers live

All three `GovernorLayer` instances are constructed and applied inside `auth::router()` in
`signalnode-api/src/auth/mod.rs`. `lib.rs` is unchanged — the rate limiting is fully encapsulated
in the auth module.

```
lib.rs
└── app()
    └── .nest("/auth", auth::router())   ← no change here
        └── auth::router()
            ├── POST /register  .layer(register_governor)
            ├── POST /login     .layer(login_governor)
            └── POST /refresh   .layer(refresh_governor)
```

### Config construction (production)

Each route gets its own `GovernorConfig<PeerIpKeyExtractor>` built with
`GovernorConfigBuilder`, wrapped in `Arc`, and passed to `GovernorLayer`:

```rust
// example — exact API may differ slightly by tower_governor version
let register_config = Arc::new(
    GovernorConfigBuilder::default()
        .per_second(12)
        .burst_size(5)
        .finish()
        .unwrap(),
);
let register_governor = GovernorLayer { config: register_config };
```

The three configs are built once when `auth::router()` is called, i.e. once at server startup.

### IP extraction

`PeerIpKeyExtractor` reads `ConnectInfo<SocketAddr>` from request extensions. In production,
axum's TCP server inserts this automatically. If the extension is absent the layer returns 500
(governor error), so every test request that hits an auth route must supply it (see Tests section).

Trusted-proxy / `X-Forwarded-For` support is explicitly out of scope for this phase.

---

## Files Changed

| File | Change |
|---|---|
| `signalnode-api/Cargo.toml` | Add `tower_governor` (version TBD, axum 0.8 compatible); promote `tower` to prod dep |
| `signalnode-api/src/auth/mod.rs` | Import governor types; build three configs; apply `.layer()` per route; update `post_json` test helper; add `tight_app` and three 429 tests |

No other files are modified.

---

## Tests

### Existing tests — no behavioral change

`post_json()` creates a fresh `app()` via `.oneshot()` for each call. A fresh app means empty
in-memory rate-limit state, so no existing test ever hits 429 regardless of configured limits.

**Required fix:** Add `ConnectInfo(127.0.0.1:0)` as a request extension inside `post_json()` so
`PeerIpKeyExtractor` finds it. Without this, all auth requests in tests return 500.

```rust
Request::builder()
    .method(Method::POST)
    .uri(uri)
    .header(header::CONTENT_TYPE, "application/json")
    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))  // ← add this
    .body(...)
```

### New 429 tests — three, one per route

A `#[cfg(test)]` function `tight_app(pool: PgPool) -> Router` builds the auth router with
`burst_size(1)` and `per_second(3600)` (one token per hour). After the first request consumes the
burst, the second is immediately rate-limited with no sleep required.

Because `GovernorLayer` holds the rate-limiter state behind `Arc`, cloning the `Router` shares the
same token bucket. The two `.oneshot()` calls on clones of the same router instance see the same
state:

```rust
#[tokio::test]
async fn login_rate_limited_returns_429() {
    let pool = PgPool::connect_lazy("postgres://unused").unwrap();
    let app = tight_app(pool);

    // First request passes through the governor (handler may return anything)
    let r1 = app.clone().oneshot(make_login_req()).await.unwrap();
    assert_ne!(r1.status(), StatusCode::TOO_MANY_REQUESTS);

    // Second request: burst exhausted, governor returns 429 before reaching handler
    let r2 = app.clone().oneshot(make_login_req()).await.unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

Same pattern for `register_rate_limited_returns_429` and `refresh_rate_limited_returns_429`.
`tight_app` does not need a real database connection because the governor rejects before the
handler (which would need the pool) is ever called for the second request. `PgPool::connect_lazy`
is sufficient.

---

## Out of Scope

- Trusted-proxy / `X-Forwarded-For` header support
- Structured JSON error bodies on 429 (separate debt item)
- Account lockout after failed logins (requires HITL review per handoff)
- Expiry-based cleanup of `refresh_tokens` table (separate debt item)
- Redis-backed distributed rate limiting
