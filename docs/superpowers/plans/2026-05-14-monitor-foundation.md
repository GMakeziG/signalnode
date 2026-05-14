# Monitor Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add workspace-scoped monitor CRUD (create + list) behind auth + membership guards, backed by a `monitors` table migration and DB tests.

**Architecture:** New `signalnode-api/src/monitor/mod.rs` module mirroring the workspace pattern. A private `check_membership` helper verifies the caller is a member of the target workspace before any DB operation; returns 403 if member row is absent, 404 if the workspace itself doesn't exist. Routes registered in `lib.rs` under the existing protected router.

**Tech Stack:** Rust, Axum 0.8 (`{param}` path syntax), sqlx 0.8, `#[sqlx::test]` for DB-backed tests, Postgres

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `migrations/20260514000004_monitors.sql` | `monitors` table + workspace_id index |
| Create | `signalnode-api/src/monitor/mod.rs` | Router, types, handlers, membership guard, tests |
| Modify | `signalnode-api/src/lib.rs` | `pub mod monitor` + nest monitor router under `/api` |

---

## Test Command

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

All `#[sqlx::test]` tests require a running Postgres instance at that URL. `#[tokio::test]` auth tests do not.

---

## Task 1: Migration

**Files:**
- Create: `migrations/20260514000004_monitors.sql`

- [ ] **Step 1: Write migration**

Create `migrations/20260514000004_monitors.sql`:

```sql
CREATE TABLE monitors (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          TEXT        NOT NULL,
    url           TEXT        NOT NULL,
    interval_secs INT         NOT NULL CHECK (interval_secs > 0),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX monitors_workspace_id_idx ON monitors (workspace_id);
```

- [ ] **Step 2: Commit**

```bash
git add migrations/20260514000004_monitors.sql
git commit -m "feat: add monitors migration"
```

---

## Task 2: Module Scaffold + Auth Rejection Tests

**Files:**
- Create: `signalnode-api/src/monitor/mod.rs`
- Modify: `signalnode-api/src/lib.rs`

- [ ] **Step 1: Create module scaffold**

Create `signalnode-api/src/monitor/mod.rs`:

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

#[derive(Serialize, sqlx::FromRow)]
struct Monitor {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    url: String,
    interval_secs: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct CreateMonitorRequest {
    name: String,
    url: String,
    interval_secs: i32,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{workspace_id}/monitors", post(create_monitor))
        .route("/workspaces/{workspace_id}/monitors", get(list_monitors))
}

async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode> {
    todo!()
}

async fn create_monitor(
    State(_state): State<AppState>,
    _current_user: CurrentUser,
    Path(_workspace_id): Path<Uuid>,
    Json(_body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn list_monitors(
    State(_state): State<AppState>,
    _current_user: CurrentUser,
    Path(_workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}
```

- [ ] **Step 2: Register module in lib.rs**

In `signalnode-api/src/lib.rs`, add `pub mod monitor;` at the top alongside the other modules, and nest the monitor router under `/api` in the protected router:

```rust
pub mod auth;
pub mod middleware;
pub mod monitor;
pub mod workspace;
```

```rust
let protected = Router::new()
    .route("/api/me", get(me))
    .nest("/api", workspace::router())
    .nest("/api", monitor::router())
    .route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        middleware::auth_middleware,
    ));
```

- [ ] **Step 3: Write auth rejection tests (unauthenticated)**

Add a `#[cfg(test)]` block at the bottom of `signalnode-api/src/monitor/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use serde_json::json;
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::{app, auth::token::encode_access_token};

    const TEST_JWT_SECRET: &str = "test-secret-at-least-32-chars-long!";

    async fn create_test_user(pool: &PgPool) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("user-{}@test.com", user_id))
            .bind("not-a-real-hash")
            .execute(pool)
            .await
            .unwrap();
        user_id
    }

    async fn create_test_workspace(pool: &PgPool, user_id: Uuid) -> Uuid {
        let workspace_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workspaces (id, name, slug, owner_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(workspace_id)
        .bind("Test Workspace")
        .bind(format!("ws-{}", workspace_id))
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        )
        .bind(workspace_id)
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
        workspace_id
    }

    async fn authed(
        pool: PgPool,
        method: Method,
        uri: &str,
        user_id: Uuid,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let token = encode_access_token(&user_id.to_string(), TEST_JWT_SECRET).unwrap();
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {token}"));
        let req = match body {
            Some(b) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&b).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        app(pool, TEST_JWT_SECRET.to_string()).oneshot(req).await.unwrap()
    }

    // --- auth rejection tests ---

    #[tokio::test]
    async fn create_monitor_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let wid = Uuid::new_v4();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&format!("/api/workspaces/{wid}/monitors"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"name":"X","url":"https://x.com","interval_secs":60}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_monitors_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let wid = Uuid::new_v4();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(&format!("/api/workspaces/{wid}/monitors"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
```

- [ ] **Step 4: Run auth tests — expect PASS**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  monitor::tests::create_monitor_unauthenticated \
  monitor::tests::list_monitors_unauthenticated
```

Expected: both PASS (middleware rejects before hitting stub handlers).

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/monitor/mod.rs signalnode-api/src/lib.rs
git commit -m "feat: scaffold monitor module with auth rejection tests"
```

---

## Task 3: check_membership Helper

**Files:**
- Modify: `signalnode-api/src/monitor/mod.rs`

- [ ] **Step 1: Write failing membership tests**

Add these tests inside the `#[cfg(test)] mod tests` block (after the existing auth tests):

```rust
    // --- membership guard tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn create_monitor_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;
        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors"),
            uid2,
            Some(json!({"name": "My Monitor", "url": "https://example.com", "interval_secs": 60})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_monitor_workspace_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = Uuid::new_v4();
        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            Some(json!({"name": "My Monitor", "url": "https://example.com", "interval_secs": 60})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
```

- [ ] **Step 2: Run tests — expect FAIL**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  monitor::tests::create_monitor_not_member \
  monitor::tests::create_monitor_workspace_not_found
```

Expected: FAIL — handlers return 501, not 403/404. (Tests may also panic on `todo!()` in `check_membership`.)

- [ ] **Step 3: Implement check_membership**

Replace the `todo!()` stub in `check_membership`:

```rust
async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode> {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => {
            match sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)",
            )
            .bind(workspace_id)
            .fetch_one(pool)
            .await
            {
                Ok(true) => Err(StatusCode::FORBIDDEN),
                Ok(false) => Err(StatusCode::NOT_FOUND),
                Err(e) => {
                    tracing::error!(error = ?e, "failed to check workspace existence");
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace membership");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
```

Wire it into both stub handlers (so tests can exercise it before full handler impl):

```rust
async fn create_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Json(_body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn list_monitors(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }
    StatusCode::NOT_IMPLEMENTED.into_response()
}
```

- [ ] **Step 4: Run membership tests — expect PASS**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  monitor::tests::create_monitor_not_member \
  monitor::tests::create_monitor_workspace_not_found
```

Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/monitor/mod.rs
git commit -m "feat: implement check_membership guard (403/404)"
```

---

## Task 4: POST /api/workspaces/{workspace_id}/monitors

**Files:**
- Modify: `signalnode-api/src/monitor/mod.rs`

- [ ] **Step 1: Write failing create tests**

Add inside `mod tests`:

```rust
    // --- create_monitor tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn create_monitor_success(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            Some(json!({"name": "My Monitor", "url": "https://example.com", "interval_secs": 60})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "My Monitor");
        assert_eq!(json["url"], "https://example.com");
        assert_eq!(json["interval_secs"], 60);
        assert_eq!(json["workspace_id"], wid.to_string());
        assert!(json["id"].is_string());
        assert!(json["created_at"].is_string());
        assert!(json["updated_at"].is_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_monitor_invalid_body(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        for body in &[
            json!({"name": "", "url": "https://example.com", "interval_secs": 60}),
            json!({"name": "Test", "url": "", "interval_secs": 60}),
            json!({"name": "Test", "url": "https://example.com", "interval_secs": 0}),
        ] {
            let res = authed(
                pool.clone(),
                Method::POST,
                &format!("/api/workspaces/{wid}/monitors"),
                uid,
                Some(body.clone()),
            )
            .await;
            assert_eq!(
                res.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "body {body:?} should be 422"
            );
        }
    }
```

- [ ] **Step 2: Run tests — expect FAIL**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  monitor::tests::create_monitor_success \
  monitor::tests::create_monitor_invalid_body
```

Expected: FAIL — handlers return 501.

- [ ] **Step 3: Implement create_monitor handler**

Replace the stub `create_monitor`:

```rust
async fn create_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    if body.name.is_empty() || body.url.is_empty() || body.interval_secs < 1 {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    match sqlx::query_as::<_, Monitor>(
        "INSERT INTO monitors (workspace_id, name, url, interval_secs)
         VALUES ($1, $2, $3, $4)
         RETURNING id, workspace_id, name, url, interval_secs, created_at, updated_at",
    )
    .bind(workspace_id)
    .bind(&body.name)
    .bind(&body.url)
    .bind(body.interval_secs)
    .fetch_one(&state.pool)
    .await
    {
        Ok(monitor) => (StatusCode::CREATED, Json(monitor)).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert monitor");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

- [ ] **Step 4: Run create tests — expect PASS**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  monitor::tests::create_monitor
```

Expected: `create_monitor_success`, `create_monitor_invalid_body`, `create_monitor_not_member`, `create_monitor_workspace_not_found`, `create_monitor_unauthenticated` all PASS.

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/monitor/mod.rs
git commit -m "feat: implement POST /api/workspaces/{workspace_id}/monitors"
```

---

## Task 5: GET /api/workspaces/{workspace_id}/monitors

**Files:**
- Modify: `signalnode-api/src/monitor/mod.rs`

- [ ] **Step 1: Write failing list tests**

Add inside `mod tests`:

```rust
    // --- list_monitors tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn list_monitors_returns_workspace_monitors(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            Some(json!({"name": "Monitor A", "url": "https://a.com", "interval_secs": 30})),
        )
        .await;
        // second workspace owned by a different user — its monitor must not appear
        let uid2 = create_test_user(&pool).await;
        let wid2 = create_test_workspace(&pool, uid2).await;
        authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid2}/monitors"),
            uid2,
            Some(json!({"name": "Monitor B", "url": "https://b.com", "interval_secs": 60})),
        )
        .await;
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "Monitor A");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_monitors_empty(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_monitors_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors"),
            uid2,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_monitors_workspace_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = Uuid::new_v4();
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
```

- [ ] **Step 2: Run list tests — expect FAIL**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  monitor::tests::list_monitors
```

Expected: FAIL — stub returns 501.

- [ ] **Step 3: Implement list_monitors handler**

Replace the stub `list_monitors`:

```rust
async fn list_monitors(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    match sqlx::query_as::<_, Monitor>(
        "SELECT id, workspace_id, name, url, interval_secs, created_at, updated_at
         FROM monitors
         WHERE workspace_id = $1
         ORDER BY created_at ASC",
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(monitors) => Json(monitors).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to list monitors");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

- [ ] **Step 4: Run all monitor tests — expect all PASS**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all tests PASS (existing 32 + 10 new monitor tests = 42 total).

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/monitor/mod.rs
git commit -m "feat: implement GET /api/workspaces/{workspace_id}/monitors"
```
