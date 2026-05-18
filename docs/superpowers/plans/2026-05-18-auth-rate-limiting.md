# Auth Rate Limiting (tower-governor) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-route IP-based rate limiting to `/auth/login` (10/min), `/auth/register` (5/min), and `/auth/refresh` (30/min) using `tower_governor`, with integration tests that assert the 429 path.

**Architecture:** Three `GovernorLayer` instances — each wrapping an `Arc<GovernorConfig<PeerIpKeyExtractor>>` — applied per-route via `MethodRouter::layer()` inside `auth::router()`. `lib.rs` is untouched. All test changes stay inside `auth/mod.rs`.

**Tech Stack:** `tower_governor` (axum 0.8 compatible release), `axum 0.8`, `tower 0.5`, `std::sync::Arc`, `std::net::SocketAddr`, `axum::extract::ConnectInfo`

---

## File Map

| File | Change |
|---|---|
| `signalnode-api/Cargo.toml` | Add `tower_governor` (version resolved in Task 1); move `tower` to `[dependencies]` |
| `signalnode-api/src/auth/mod.rs` | Governor imports + `Arc`; three `GovernorLayer` per route in `router()`; `ConnectInfo` fix in `post_json`; `tight_app` helper + three 429 tests |

No other files are modified.

---

## Task 1: Resolve tower_governor version and add dependencies

**Files:**
- Modify: `signalnode-api/Cargo.toml`

- [ ] **Step 1.1 — Find the axum 0.8 compatible release**

```bash
cargo search tower_governor
```

Look at the version numbers returned. Then verify the one you intend to use actually requires axum 0.8 by checking its published `Cargo.toml` on crates.io (`https://crates.io/crates/tower_governor/<VERSION>/dependencies`). You want a release whose dependency list shows `axum = "^0.8"` (or a compatible range) and `tower = "^0.5"`. The Phase 4 handoff cited `"0.4"` which targets axum 0.7 — do not use it.

Write the resolved version string down before editing any file.

- [ ] **Step 1.2 — Update `signalnode-api/Cargo.toml`**

Current file (relevant sections):
```toml
[dependencies]
argon2 = "0.5"
axum = { version = "0.8", features = ["macros"] }
chrono = { version = "0.4", features = ["serde"] }
jsonwebtoken = "9"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono", "macros"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
uuid = { version = "1", features = ["v4", "serde"] }

[dev-dependencies]
tower = { version = "0.5", features = ["util"] }
```

Replace with:
```toml
[dependencies]
argon2 = "0.5"
axum = { version = "0.8", features = ["macros"] }
chrono = { version = "0.4", features = ["serde"] }
jsonwebtoken = "9"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono", "macros"] }
tokio = { version = "1", features = ["full"] }
tower = { version = "0.5", features = ["util"] }
tower_governor = "REPLACE_WITH_VERSION_FROM_STEP_1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
uuid = { version = "1", features = ["v4", "serde"] }

[dev-dependencies]
# tower promoted to [dependencies]; no dev-only deps remain unless you add them
```

- [ ] **Step 1.3 — Verify the dependency resolves**

```bash
cargo check --manifest-path signalnode-api/Cargo.toml
```

Expected: compiles cleanly with no errors. If `cargo` reports a version conflict between `tower` (now in `[dependencies]`) and `tower_governor`'s transitive `tower` dep, tighten both to the same minor: `tower = "0.5"` and `tower_governor`'s required minor should match. Adjust until `cargo check` is clean.

- [ ] **Step 1.4 — Commit**

```bash
git add signalnode-api/Cargo.toml signalnode-api/Cargo.lock
git commit -m "deps(api): add tower_governor; promote tower to prod dep"
```

---

## Task 2: Write failing 429 tests (red phase)

**Files:**
- Modify: `signalnode-api/src/auth/mod.rs` — test section only

No production code changes in this task. Existing 151 tests continue to pass. The three new 429 tests will fail at the `assert_eq!(r2.status(), TOO_MANY_REQUESTS)` line because `tight_app` at this stage has no governors.

- [ ] **Step 2.1 — Add imports to `mod tests`**

Inside `mod tests { ... }` at the top of the existing `use` block, add:

```rust
use axum::extract::ConnectInfo;
use std::net::SocketAddr;
```

- [ ] **Step 2.2 — Add the `tight_app` stub**

Inside `mod tests`, before the first `#[sqlx::test]` function, add:

```rust
fn tight_app(pool: PgPool) -> Router {
    let state = crate::AppState {
        pool,
        jwt_secret: TEST_JWT_SECRET.to_string(),
    };
    Router::new()
        .route("/login", post(super::login))
        .route("/register", post(super::register))
        .route("/refresh", post(super::refresh))
        .with_state(state)
}
```

This stub has no governors. It exists so the 429 tests compile and run, producing the correct failure (422 for r2 instead of 429).

- [ ] **Step 2.3 — Add `login_rate_limited_returns_429`**

Inside `mod tests`:

```rust
#[tokio::test]
async fn login_rate_limited_returns_429() {
    let pool = PgPool::connect_lazy("postgres://unused").unwrap();
    let app = tight_app(pool);

    // Governor passes r1 through; axum's Json extractor rejects {} (missing
    // email + password fields) → 422. No DB connection needed.
    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Burst exhausted; governor intercepts r2 before extractors run → 429.
    let r2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

- [ ] **Step 2.4 — Add `register_rate_limited_returns_429`**

```rust
#[tokio::test]
async fn register_rate_limited_returns_429() {
    let pool = PgPool::connect_lazy("postgres://unused").unwrap();
    let app = tight_app(pool);

    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let r2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

- [ ] **Step 2.5 — Add `refresh_rate_limited_returns_429`**

```rust
#[tokio::test]
async fn refresh_rate_limited_returns_429() {
    let pool = PgPool::connect_lazy("postgres://unused").unwrap();
    let app = tight_app(pool);

    let r1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/refresh")
                .header(header::CONTENT_TYPE, "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let r2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/refresh")
                .header(header::CONTENT_TYPE, "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

- [ ] **Step 2.6 — Run the new tests and confirm they fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml -- rate_limited 2>&1 | tail -20
```

Expected: all three new tests fail at the r2 assertion:

```
thread 'auth::tests::login_rate_limited_returns_429' panicked at ...
assertion `left == right` failed
  left: 422
 right: 429
```

This is correct. The tests are red because `tight_app` has no governors yet.

- [ ] **Step 2.7 — Confirm existing tests still pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: 151 passed, 3 failed (the three rate-limit tests). No regressions.

- [ ] **Step 2.8 — Commit the red state**

```bash
git add signalnode-api/src/auth/mod.rs
git commit -m "test(auth): add failing 429 rate-limit tests (red)"
```

---

## Task 3: Wire GovernorLayer — production router + tight_app (green phase)

**Files:**
- Modify: `signalnode-api/src/auth/mod.rs` — production section + test section

- [ ] **Step 3.1 — Add governor imports to the production section of `auth/mod.rs`**

At the top of `auth/mod.rs`, alongside the existing `use` statements (outside `mod tests`), add:

```rust
use std::sync::Arc;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
```

`Arc` and `GovernorConfigBuilder`/`GovernorLayer` are needed both in `router()` and in `tight_app` (which is in `mod tests`). Since `mod tests` is a child module, it sees these imports through `super`.

- [ ] **Step 3.2 — Replace `router()` with the governed version**

The current body of `pub fn router()` in `auth/mod.rs` (lines ~51–55):

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/refresh", post(refresh))
}
```

Replace it with:

```rust
pub fn router() -> Router<AppState> {
    let register_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(12)
            .burst_size(5)
            .finish()
            .unwrap(),
    );
    let login_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(6)
            .burst_size(10)
            .finish()
            .unwrap(),
    );
    let refresh_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(2)
            .burst_size(30)
            .finish()
            .unwrap(),
    );

    Router::new()
        .route("/register", post(register).layer(GovernorLayer::new(register_config)))
        .route("/login", post(login).layer(GovernorLayer::new(login_config)))
        .route("/refresh", post(refresh).layer(GovernorLayer::new(refresh_config)))
}
```

Rate math for reference: `per_second(N)` replenishes one token every N seconds.
- register: 1 per 12 s = 5/min, burst 5 → 6th request in window is rejected
- login: 1 per 6 s = 10/min, burst 10 → 11th request in window is rejected
- refresh: 1 per 2 s = 30/min, burst 30 → 31st request in window is rejected

- [ ] **Step 3.3 — Fix `post_json` in `mod tests`**

The `ConnectInfo` and `SocketAddr` imports are already present from Task 2. Update only the body of `post_json`:

Current:
```rust
async fn post_json(
    pool: PgPool,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let app = app(pool, TEST_JWT_SECRET.to_string());
    app.oneshot(
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}
```

Replace with:
```rust
async fn post_json(
    pool: PgPool,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let app = app(pool, TEST_JWT_SECRET.to_string());
    app.oneshot(
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}
```

Without this fix, every existing auth test returns 500 because `PeerIpKeyExtractor` can't find `ConnectInfo` in the request extensions and the governor rejects the request as an internal error.

- [ ] **Step 3.4 — Replace `tight_app` stub with the real implementation**

First, add governor imports inside `mod tests` (Rust `use` declarations are module-scoped and do not bleed from parent to child modules — `tight_app` can't see the imports added in Step 3.1 without this):

```rust
// Add these alongside the ConnectInfo/SocketAddr imports already present in mod tests
use std::sync::Arc;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
```

Then replace the stub from Task 2 with:

```rust
fn tight_app(pool: PgPool) -> Router {
    let state = crate::AppState {
        pool,
        jwt_secret: TEST_JWT_SECRET.to_string(),
    };
    let register_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(3600)
            .burst_size(1)
            .finish()
            .unwrap(),
    );
    let login_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(3600)
            .burst_size(1)
            .finish()
            .unwrap(),
    );
    let refresh_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(3600)
            .burst_size(1)
            .finish()
            .unwrap(),
    );
    Router::new()
        .route("/login", post(super::login).layer(GovernorLayer::new(login_config)))
        .route("/register", post(super::register).layer(GovernorLayer::new(register_config)))
        .route("/refresh", post(super::refresh).layer(GovernorLayer::new(refresh_config)))
        .with_state(state)
}
```

`per_second(3600)` = one token per hour. With `burst_size(1)`, the first request consumes the only burst token; the second request is always rejected immediately without any sleep.

Each route gets its own `Arc<GovernorConfig>` so their token buckets are independent.

- [ ] **Step 3.5 — Run all tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -10
```

Expected:
```
test result: ok. 154 passed; 0 failed; 0 ignored; ...
```

**If existing tests fail with 500:** `PeerIpKeyExtractor` couldn't find `ConnectInfo`. Check that `post_json` has the `.extension(ConnectInfo(...))` line and that the `use` imports are present.

**If 429 tests fail (r2 returns 422 not 429):** `Router::clone()` is not sharing governor state. Verify that `GovernorLayer` in this version implements `Clone` by cloning the inner `Arc` rather than creating a fresh rate limiter. If it doesn't, you can work around it by calling `app.into_service()` and calling the service directly — but check the tower_governor changelog first; this is a library contract that should hold.

- [ ] **Step 3.6 — Commit**

```bash
git add signalnode-api/src/auth/mod.rs
git commit -m "feat(auth): per-route rate limiting via tower_governor

login 10/min (burst 10), register 5/min (burst 5), refresh 30/min (burst 30).
PeerIpKeyExtractor reads ConnectInfo<SocketAddr>. post_json test helper
updated to inject ConnectInfo so existing 151 tests are unaffected.
tight_app helper uses burst=1 to assert 429 without sleeps."
```

---

## Task 4: Acceptance check

- [ ] **Step 4.1 — Confirm test count**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | grep "^test result"
```

Expected:
```
test result: ok. 154 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

- [ ] **Step 4.2 — signalnode-core regression check**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml 2>&1 | grep "^test result"
```

Expected:
```
test result: ok. 29 passed; 0 failed; ...
```

- [ ] **Step 4.3 — Run clippy**

```bash
cargo clippy --manifest-path signalnode-api/Cargo.toml -- -D warnings
```

Expected: no output. If clippy warns about `Arc::new(...)` patterns or `unwrap()` on `GovernorConfigBuilder::finish()`, use `expect("valid governor config")` on the `.unwrap()` calls.

- [ ] **Step 4.4 — Verify `/health` is unaffected**

The `health_returns_200` test in `lib.rs` does not hit any auth route and requires no `ConnectInfo`. Confirm it passed in Step 4.1.
