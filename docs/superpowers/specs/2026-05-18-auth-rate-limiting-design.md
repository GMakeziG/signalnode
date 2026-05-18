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

| Route | Tokens / min | `per_second(N)` ¹ | `burst_size` | Rationale |
|---|---|---|---|---|
| `POST /auth/register` | 5 | `per_second(12)` | 5 | Account creation is rarely burst-legitimate |
| `POST /auth/login` | 10 | `per_second(6)` | 10 | Allows a few fast retries before throttling |
| `POST /auth/refresh` | 30 | `per_second(2)` | 30 | Background token rotation is more frequent |

¹ `per_second(N)` means one token replenished every N seconds (N is the period, not the rate).
A burst of B tokens means up to B requests are allowed before replenishment kicks in.

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

A `#[cfg(test)]` function `tight_app(pool: PgPool) -> Router` (inside `auth/mod.rs`) builds a
standalone router — **no `/auth/` nesting prefix** — with `burst_size(1)` and `per_second(3600)`
(one token per hour) for each route. Routes are exposed at `/login`, `/register`, `/refresh`.
Each route gets its own `GovernorConfig` instance (mirroring production layout) so the three
token buckets are independent.

After the first request consumes the burst token, the second is immediately rate-limited with no
sleep required.

Because `GovernorLayer` holds the rate-limiter state behind `Arc`, cloning the `Router` shares the
same token bucket. The two `.oneshot()` calls on clones of the same router instance see the same
state.

> **Implementation note:** Verify at implementation time that `Router::clone()` in axum 0.8 does
> share the inner `Arc<GovernorConfig>` state rather than creating a fresh copy. If cloning were to
> produce independent state, r2 would return 422 (not 429) and the assertion would fail loudly —
> but it is better to confirm the behaviour is intentional than to rely on a silent assumption.

Each request body is `{}` — an empty JSON object. All three request structs (`LoginRequest`,
`RegisterRequest`, `RefreshRequest`) have required string fields with no defaults. Axum's
`Json<T>` extractor rejects `{}` with 422 *before* the handler body runs, so no database
connection is reached for r1 either. `PgPool::connect_lazy("postgres://unused")` is sufficient
for the entire test.

```rust
#[tokio::test]
async fn login_rate_limited_returns_429() {
    let pool = PgPool::connect_lazy("postgres://unused").unwrap();
    let app = tight_app(pool);

    // Governor passes r1 through; extractor rejects {} (missing required fields) → 422.
    // This confirms the governor did not block the request.
    let r1 = app.clone()
        .oneshot(Request::builder()
            .method(Method::POST).uri("/login")
            .header(header::CONTENT_TYPE, "application/json")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
            .body(Body::from("{}")).unwrap())
        .await.unwrap();
    assert_eq!(r1.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Burst exhausted; governor intercepts r2 before extractors run → 429.
    let r2 = app.clone()
        .oneshot(Request::builder()
            .method(Method::POST).uri("/login")
            .header(header::CONTENT_TYPE, "application/json")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
            .body(Body::from("{}")).unwrap())
        .await.unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

Same pattern for `register_rate_limited_returns_429` (`/register`) and
`refresh_rate_limited_returns_429` (`/refresh`).

---

## Acceptance Criteria

Phase 5 is done when all of the following are true:

1. `cargo test` passes — all 151 existing tests green, plus 3 new 429 tests = **154 total**.
2. `cargo clippy` is clean (no new warnings).
3. `GET /health` returns 200 — unaffected by auth rate limiting.
4. `POST /auth/login` returns 429 after 10 requests from the same IP within one minute.
5. `POST /auth/register` returns 429 after 5 requests from the same IP within one minute.
6. `POST /auth/refresh` returns 429 after 30 requests from the same IP within one minute.
7. All existing auth tests (`register_success`, `login_success_returns_tokens`, etc.) continue to
   pass — each creates a fresh app instance with empty rate-limit state, so none trigger 429.

---

## Out of Scope

- Trusted-proxy / `X-Forwarded-For` header support
- Structured JSON error bodies on 429 (separate debt item)
- Account lockout after failed logins (requires HITL review per handoff)
- Expiry-based cleanup of `refresh_tokens` table (separate debt item)
- Redis-backed distributed rate limiting
