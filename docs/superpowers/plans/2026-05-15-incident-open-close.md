# Incident Open/Close Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Open an Incident after `failure_threshold` consecutive `down` CheckResults; close it after `recovery_threshold` consecutive `up` CheckResults; expose open Incidents via `GET /api/workspaces/{workspace_id}/incidents`.

**Architecture:** Evaluation runs inline inside a database transaction in `create_check_result`. A new `incident` module owns the GET route and the `Incident` struct. No service layer — thin handlers, direct sqlx queries, consistent with existing modules.

**Tech Stack:** Rust, Axum 0.8, sqlx 0.8 (PostgreSQL), `#[sqlx::test]` integration tests

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `migrations/20260515000007_incidents.sql` | incidents table + indexes |
| Create | `signalnode-api/src/incident/mod.rs` | `Incident` struct, `router()`, GET handler, tests |
| Modify | `signalnode-api/src/lib.rs` | add `pub mod incident;` + wire router |
| Modify | `signalnode-api/src/check_result/mod.rs` | wrap insert in transaction, add open/close evaluation + tests |

---

## Test Command

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

`#[sqlx::test]` creates an isolated database per test and applies all migrations automatically. Pure-logic tests (no `pool` parameter) run without a database.

---

## Task 1: Migration — incidents table

**Files:**
- Create: `migrations/20260515000007_incidents.sql`

- [ ] **Step 1: Write the migration**

`migrations/20260515000007_incidents.sql`:

```sql
CREATE TABLE incidents (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    monitor_id UUID        NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    opened_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at  TIMESTAMPTZ
);

CREATE INDEX incidents_monitor_id_idx ON incidents (monitor_id, opened_at DESC);
CREATE INDEX incidents_open_idx       ON incidents (monitor_id) WHERE closed_at IS NULL;
```

`closed_at IS NULL` means the Incident is open. The partial index makes the "is there an open incident?" lookup fast.

- [ ] **Step 2: Run tests to confirm migration applies cleanly**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all existing tests pass (the new migration is applied by `#[sqlx::test]` automatically).

- [ ] **Step 3: Commit**

```bash
git add migrations/20260515000007_incidents.sql
git commit -m "feat: add incidents table migration"
```

---

## Task 2: Incident module skeleton + lib.rs wiring

**Files:**
- Create: `signalnode-api/src/incident/mod.rs`
- Modify: `signalnode-api/src/lib.rs`

- [ ] **Step 1: Create the module file**

`signalnode-api/src/incident/mod.rs`:

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

#[derive(Serialize, sqlx::FromRow)]
struct Incident {
    id: Uuid,
    monitor_id: Uuid,
    opened_at: DateTime<Utc>,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/workspaces/{workspace_id}/incidents",
        get(list_open_incidents),
    )
}

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

async fn list_open_incidents(
    _state: State<AppState>,
    _current_user: CurrentUser,
    _path: Path<Uuid>,
) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}
```

- [ ] **Step 2: Wire the module into lib.rs**

In `signalnode-api/src/lib.rs`, add `pub mod incident;` alongside the other module declarations, and add `.nest("/api", incident::router())` to the protected router.

The file after changes:

```rust
pub mod auth;
pub mod check_result;
pub mod incident;
pub mod middleware;
pub mod monitor;
pub mod workspace;

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use middleware::CurrentUser;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_secret: String,
}

pub fn app(pool: PgPool, jwt_secret: String) -> Router {
    assert!(!jwt_secret.is_empty(), "JWT_SECRET must be set and non-empty");
    let state = AppState { pool, jwt_secret };

    let protected = Router::new()
        .route("/api/me", get(me))
        .nest("/api", workspace::router())
        .nest("/api", monitor::router())
        .nest("/api", check_result::router())
        .nest("/api", incident::router())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ));

    Router::new()
        .route("/health", get(health))
        .nest("/auth", auth::router())
        .merge(protected)
        .with_state(state)
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn me(current_user: CurrentUser) -> impl IntoResponse {
    Json(serde_json::json!({ "id": current_user.id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::auth::token::{encode_access_token, encode_refresh_token};

    const TEST_JWT: &str = "test-secret-at-least-32-chars-long!";
    const TEST_UID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn test_app() -> Router {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        app(pool, TEST_JWT.to_string())
    }

    #[tokio::test]
    async fn health_returns_200() {
        let response = test_app()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn me_missing_token_returns_401() {
        let res = test_app()
            .oneshot(Request::builder().uri("/api/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_invalid_token_returns_401() {
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("Authorization", "Bearer notavalidtoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_refresh_token_rejected() {
        let token = encode_refresh_token(TEST_UID, TEST_JWT).unwrap();
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_valid_access_token_accepted() {
        let token = encode_access_token(TEST_UID, TEST_JWT).unwrap();
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], TEST_UID);
    }
}
```

- [ ] **Step 3: Run tests to confirm it compiles and existing tests still pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all existing tests pass. The stub handler returns 501 but no tests hit it yet.

- [ ] **Step 4: Commit**

```bash
git add signalnode-api/src/incident/mod.rs signalnode-api/src/lib.rs
git commit -m "feat: add incident module skeleton and wire router"
```

---

## Task 3: GET /api/workspaces/{workspace_id}/incidents — tests + handler

**Files:**
- Modify: `signalnode-api/src/incident/mod.rs`

- [ ] **Step 1: Add test helpers and failing tests**

Append a `#[cfg(test)]` block to `signalnode-api/src/incident/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
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

    async fn create_test_monitor(pool: &PgPool, workspace_id: Uuid) -> Uuid {
        let monitor_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO monitors (id, workspace_id, name, url, interval_secs) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(monitor_id)
        .bind(workspace_id)
        .bind("Test Monitor")
        .bind("https://example.com")
        .bind(60_i32)
        .execute(pool)
        .await
        .unwrap();
        monitor_id
    }

    async fn create_open_incident(pool: &PgPool, monitor_id: Uuid) -> Uuid {
        let incident_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO incidents (id, monitor_id) VALUES ($1, $2)",
        )
        .bind(incident_id)
        .bind(monitor_id)
        .execute(pool)
        .await
        .unwrap();
        incident_id
    }

    async fn authed(
        pool: PgPool,
        method: Method,
        uri: &str,
        user_id: Uuid,
    ) -> axum::response::Response {
        let token = encode_access_token(&user_id.to_string(), TEST_JWT_SECRET).unwrap();
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap();
        app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(req)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_open_incidents_empty(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/incidents"),
            uid,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_open_incidents_returns_open_only(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        // One open incident
        let open_id = create_open_incident(&pool, mid).await;

        // One closed incident
        sqlx::query(
            "INSERT INTO incidents (monitor_id, opened_at, closed_at) \
             VALUES ($1, NOW() - INTERVAL '10 minutes', NOW() - INTERVAL '5 minutes')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/incidents"),
            uid,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], open_id.to_string());
        assert!(arr[0]["opened_at"].is_string());
        assert!(arr[0]["monitor_id"].is_string());
        // closed_at must NOT appear in the response shape
        assert!(arr[0].get("closed_at").is_none());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_open_incidents_scoped_to_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid1 = create_test_workspace(&pool, uid).await;
        let wid2 = create_test_workspace(&pool, uid).await;
        let mid1 = create_test_monitor(&pool, wid1).await;
        let mid2 = create_test_monitor(&pool, wid2).await;

        create_open_incident(&pool, mid1).await;
        create_open_incident(&pool, mid2).await;

        // GET workspace 1 — should only see mid1's incident
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid1}/incidents"),
            uid,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["monitor_id"], mid1.to_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_open_incidents_ordered_newest_first(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid1 = create_test_monitor(&pool, wid).await;
        let mid2 = create_test_monitor(&pool, wid).await;

        let older_id = Uuid::new_v4();
        let newer_id = Uuid::new_v4();

        sqlx::query(
            "INSERT INTO incidents (id, monitor_id, opened_at) VALUES ($1, $2, NOW() - INTERVAL '10 minutes')",
        )
        .bind(older_id)
        .bind(mid1)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO incidents (id, monitor_id, opened_at) VALUES ($1, $2, NOW())",
        )
        .bind(newer_id)
        .bind(mid2)
        .execute(&pool)
        .await
        .unwrap();

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/incidents"),
            uid,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], newer_id.to_string());
        assert_eq!(arr[1]["id"], older_id.to_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_open_incidents_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/incidents"),
            uid2,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_open_incidents_wrong_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let _wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{}/incidents", Uuid::new_v4()),
            uid,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_open_incidents_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(&format!("/api/workspaces/{}/incidents", Uuid::new_v4()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml incident
```

Expected: the new tests FAIL (stub handler returns 501, not 200/403/404).

- [ ] **Step 3: Implement the GET handler**

Replace the stub `list_open_incidents` function in `signalnode-api/src/incident/mod.rs`:

```rust
async fn list_open_incidents(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    match sqlx::query_as::<_, Incident>(
        "SELECT i.id, i.monitor_id, i.opened_at
         FROM incidents i
         JOIN monitors m ON m.id = i.monitor_id
         WHERE m.workspace_id = $1
           AND i.closed_at IS NULL
         ORDER BY i.opened_at DESC",
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(incidents) => Json(incidents).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to list open incidents");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/incident/mod.rs
git commit -m "feat: add GET /workspaces/{id}/incidents — open incidents only"
```

---

## Task 4: Incident open logic — tests + implementation

**Files:**
- Modify: `signalnode-api/src/check_result/mod.rs`

- [ ] **Step 1: Add failing tests for the open path**

Add these tests inside the existing `#[cfg(test)] mod tests` block in `signalnode-api/src/check_result/mod.rs`, after the existing `list_check_results_not_member` test and before the closing `}` of the test module. Also add the `create_test_monitor_thresholds` helper alongside the existing helpers:

```rust
    async fn create_test_monitor_thresholds(
        pool: &PgPool,
        workspace_id: Uuid,
        failure_threshold: i32,
        recovery_threshold: i32,
    ) -> Uuid {
        let monitor_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO monitors (id, workspace_id, name, url, interval_secs, failure_threshold, recovery_threshold) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(monitor_id)
        .bind(workspace_id)
        .bind("Test Monitor")
        .bind("https://example.com")
        .bind(60_i32)
        .bind(failure_threshold)
        .bind(recovery_threshold)
        .execute(pool)
        .await
        .unwrap();
        monitor_id
    }

    async fn open_incident_count(pool: &PgPool, monitor_id: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL",
        )
        .bind(monitor_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    // --- incident open tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn open_incident_after_threshold(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 2: need 2 consecutive down
        let mid = create_test_monitor_thresholds(&pool, wid, 2, 1).await;

        // First down — threshold not yet crossed, no incident
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'down', NOW() - INTERVAL '10 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(open_incident_count(&pool, mid).await, 0);

        // Second down via API — threshold crossed, incident opens
        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(open_incident_count(&pool, mid).await, 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn no_open_below_threshold(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 2, only 1 down → should not open
        let mid = create_test_monitor_thresholds(&pool, wid, 2, 1).await;

        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(open_incident_count(&pool, mid).await, 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn degraded_does_not_count(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 2
        let mid = create_test_monitor_thresholds(&pool, wid, 2, 1).await;

        // Insert a degraded result — should interrupt any down streak
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'degraded', NOW() - INTERVAL '10 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        // One down after degraded → last 2 are [down, degraded] — not all down
        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(open_incident_count(&pool, mid).await, 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn no_duplicate_open_incident(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 1: each down opens (or attempts to open) an incident
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 2).await;

        // First down — opens incident
        authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(open_incident_count(&pool, mid).await, 1);

        // Second down — incident already open, close path runs but last result is down → no close
        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        // Still exactly 1 open incident — no duplicate
        assert_eq!(open_incident_count(&pool, mid).await, 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn paused_monitor_no_open(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 1).await;

        // Pause the monitor directly
        sqlx::query("UPDATE monitors SET status = 'paused' WHERE id = $1")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();

        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(open_incident_count(&pool, mid).await, 0);
    }
```

- [ ] **Step 2: Run tests to confirm the new ones fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml check_result
```

Expected: `open_incident_after_threshold`, `no_duplicate_open_incident`, `paused_monitor_no_open` FAIL (no incident is ever opened). `no_open_below_threshold`, `degraded_does_not_count` may pass by accident — that's acceptable; they'll be meaningful after implementation.

- [ ] **Step 3: Implement the open evaluation**

Replace the `create_check_result` function in `signalnode-api/src/check_result/mod.rs` with this transaction-wrapped version that includes open evaluation. The close branch is left as a placeholder (empty `else` block) until Task 5:

```rust
async fn create_check_result(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<CreateCheckResultRequest>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }
    if let Err(status) = resolve_monitor(&state.pool, workspace_id, monitor_id).await {
        return status.into_response();
    }

    if !matches!(body.status.as_str(), "up" | "degraded" | "down") {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "failed to begin transaction");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let cr = match sqlx::query_as::<_, CheckResult>(
        "INSERT INTO check_results (monitor_id, status, latency_ms, error_detail)
         VALUES ($1, $2, $3, $4)
         RETURNING id, monitor_id, status, latency_ms, error_detail, checked_at",
    )
    .bind(monitor_id)
    .bind(&body.status)
    .bind(body.latency_ms)
    .bind(body.error_detail.as_deref())
    .fetch_one(&mut *tx)
    .await
    {
        Ok(cr) => cr,
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert check_result");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let (monitor_status, failure_threshold, recovery_threshold) =
        match sqlx::query_as::<_, (String, i32, i32)>(
            "SELECT status, failure_threshold, recovery_threshold FROM monitors WHERE id = $1",
        )
        .bind(monitor_id)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::error!(error = ?e, "failed to fetch monitor for incident evaluation");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

    if monitor_status == "active" {
        let open_incident: Option<Uuid> = match sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL LIMIT 1",
        )
        .bind(monitor_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::error!(error = ?e, "failed to check for open incident");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        if open_incident.is_none() {
            // Open path: check if the last failure_threshold results are all 'down'
            let recent: Vec<String> = match sqlx::query_scalar::<_, String>(
                "SELECT status FROM check_results \
                 WHERE monitor_id = $1 ORDER BY checked_at DESC LIMIT $2",
            )
            .bind(monitor_id)
            .bind(failure_threshold)
            .fetch_all(&mut *tx)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!(error = ?e, "failed to fetch results for open evaluation");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };

            if recent.len() == failure_threshold as usize && recent.iter().all(|s| s == "down") {
                if let Err(e) = sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
                    .bind(monitor_id)
                    .execute(&mut *tx)
                    .await
                {
                    tracing::error!(error = ?e, "failed to open incident");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        // Close path added in Task 5
    }

    match tx.commit().await {
        Ok(_) => (StatusCode::CREATED, Json(cr)).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to commit transaction");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

- [ ] **Step 4: Run tests to confirm all pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all tests pass including the new open-path tests. The `no_duplicate_open_incident` test also passes because when an incident is already open the close branch (empty) does nothing.

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/check_result/mod.rs
git commit -m "feat: open Incident after failure_threshold consecutive down CheckResults"
```

---

## Task 5: Incident close logic — tests + implementation

**Files:**
- Modify: `signalnode-api/src/check_result/mod.rs`

- [ ] **Step 1: Add failing tests for the close path**

Add these tests inside the same `#[cfg(test)] mod tests` block in `signalnode-api/src/check_result/mod.rs`, after the `paused_monitor_no_open` test:

```rust
    async fn closed_incident_count(pool: &PgPool, monitor_id: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NOT NULL",
        )
        .bind(monitor_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    // --- incident close tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn close_incident_after_recovery(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 1, recovery_threshold = 2
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 2).await;

        // Open an incident with one down
        authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(open_incident_count(&pool, mid).await, 1);

        // First up — recovery_threshold not yet met (need 2 consecutive up)
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'up', NOW() - INTERVAL '5 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        // Second up via API — recovery_threshold = 2 met, incident closes
        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "up"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(open_incident_count(&pool, mid).await, 0);
        assert_eq!(closed_incident_count(&pool, mid).await, 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn no_close_below_recovery(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 1, recovery_threshold = 2: need 2 up to close
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 2).await;

        // Open incident
        authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "down"})),
        )
        .await;
        assert_eq!(open_incident_count(&pool, mid).await, 1);

        // Only 1 up — not enough to close (need 2)
        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(serde_json::json!({"status": "up"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(open_incident_count(&pool, mid).await, 1);
        assert_eq!(closed_incident_count(&pool, mid).await, 0);
    }
```

- [ ] **Step 2: Run tests to confirm the new ones fail**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml check_result
```

Expected: `close_incident_after_recovery` FAILS (no close logic yet, incident stays open). `no_close_below_recovery` may pass by accident — that's fine.

- [ ] **Step 3: Implement the close evaluation**

In `signalnode-api/src/check_result/mod.rs`, replace the `// Close path added in Task 5` comment with the close branch. The full `if monitor_status == "active"` block should now read:

```rust
    if monitor_status == "active" {
        let open_incident: Option<Uuid> = match sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL LIMIT 1",
        )
        .bind(monitor_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::error!(error = ?e, "failed to check for open incident");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        if open_incident.is_none() {
            // Open path: check if the last failure_threshold results are all 'down'
            let recent: Vec<String> = match sqlx::query_scalar::<_, String>(
                "SELECT status FROM check_results \
                 WHERE monitor_id = $1 ORDER BY checked_at DESC LIMIT $2",
            )
            .bind(monitor_id)
            .bind(failure_threshold)
            .fetch_all(&mut *tx)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!(error = ?e, "failed to fetch results for open evaluation");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };

            if recent.len() == failure_threshold as usize && recent.iter().all(|s| s == "down") {
                if let Err(e) = sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
                    .bind(monitor_id)
                    .execute(&mut *tx)
                    .await
                {
                    tracing::error!(error = ?e, "failed to open incident");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        } else {
            // Close path: check if the last recovery_threshold results are all 'up'
            let recent: Vec<String> = match sqlx::query_scalar::<_, String>(
                "SELECT status FROM check_results \
                 WHERE monitor_id = $1 ORDER BY checked_at DESC LIMIT $2",
            )
            .bind(monitor_id)
            .bind(recovery_threshold)
            .fetch_all(&mut *tx)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!(error = ?e, "failed to fetch results for close evaluation");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };

            if recent.len() == recovery_threshold as usize && recent.iter().all(|s| s == "up") {
                if let Err(e) = sqlx::query(
                    "UPDATE incidents SET closed_at = NOW() \
                     WHERE monitor_id = $1 AND closed_at IS NULL",
                )
                .bind(monitor_id)
                .execute(&mut *tx)
                .await
                {
                    tracing::error!(error = ?e, "failed to close incident");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }
```

- [ ] **Step 4: Run the full test suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml
```

Expected: all tests pass. Final count should be the previous total plus 9 new tests (4 open-path + 2 helpers + 2 close-path + 1 closed count helper — counted by assertions not helpers).

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/check_result/mod.rs
git commit -m "feat: close Incident after recovery_threshold consecutive up CheckResults"
```

---

## Done

After all 5 tasks, the feature is complete:

- `incidents` table with open/closed state
- Incident opens when `failure_threshold` consecutive `down` CheckResults are recorded on an active monitor
- Incident closes when `recovery_threshold` consecutive `up` CheckResults are recorded on a monitor with an open Incident
- `GET /api/workspaces/{workspace_id}/incidents` returns all open Incidents for the workspace, ordered newest first
- Full integration test coverage: open, close, threshold boundaries, `degraded` non-counting, paused monitors, auth guards

**5 commits, smallest vertical slice, no Notification dispatch, no history route.**
