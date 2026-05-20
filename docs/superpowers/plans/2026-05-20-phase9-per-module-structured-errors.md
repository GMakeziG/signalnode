# Phase 9 — Per-Module Structured Error Responses Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add structured `{"code","message"}` JSON error bodies to workspace, monitor, incident, notification_channel, and check_result API modules.

**Architecture:** Five per-module error enums (`WorkspaceError`, `MonitorError`, etc.) each implementing `IntoResponse`. A shared `ErrorBody` struct in `lib.rs` (with `Cow<'static, str>` for the message field) eliminates duplicated JSON construction. Private `check_membership`/`check_owner`/`resolve_monitor` helpers in each module change return type from `Result<(), StatusCode>` to `Result<(), ModuleError>`. HTTP status codes are unchanged; only error bodies gain structure.

**Tech Stack:** Rust, Axum, sqlx, serde, `std::borrow::Cow`

---

## File Structure

| Action | Path | Purpose |
|---|---|---|
| Modify | `src/lib.rs` | Add `pub struct ErrorBody` |
| Create | `src/workspace/error.rs` | `WorkspaceError` + `IntoResponse` |
| Modify | `src/workspace/mod.rs` | Adopt `WorkspaceError`, add body test |
| Create | `src/monitor/error.rs` | `MonitorError` + `IntoResponse` |
| Modify | `src/monitor/mod.rs` | Adopt `MonitorError`, add body test |
| Create | `src/incident/error.rs` | `IncidentError` + `IntoResponse` |
| Modify | `src/incident/mod.rs` | Adopt `IncidentError`, add body test |
| Create | `src/notification_channel/error.rs` | `NotificationChannelError` + `IntoResponse` |
| Modify | `src/notification_channel/mod.rs` | Adopt `NotificationChannelError`, add body test |
| Create | `src/check_result/error.rs` | `CheckResultError` + `IntoResponse` |
| Modify | `src/check_result/mod.rs` | Adopt `CheckResultError`, add body test |

---

## Task 1: Shared `ErrorBody` + workspace structured errors

**Files:**
- Modify: `signalnode-api/src/lib.rs`
- Create: `signalnode-api/src/workspace/error.rs`
- Modify: `signalnode-api/src/workspace/mod.rs`

- [ ] **Step 1: Write the failing body-contract test**

Add this test at the end of the `mod tests` block in `signalnode-api/src/workspace/mod.rs` (after `list_workspaces_unauthenticated`):

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_workspace_duplicate_slug_returns_structured_error(pool: PgPool) {
    let uid1 = create_test_user(&pool).await;
    authed(
        pool.clone(),
        Method::POST,
        "/api/workspaces",
        uid1,
        Some(json!({"name": "First", "slug": "same-slug"})),
    )
    .await;
    let uid2 = create_test_user(&pool).await;
    let res = authed(
        pool,
        Method::POST,
        "/api/workspaces",
        uid2,
        Some(json!({"name": "Second", "slug": "same-slug"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "slug_taken");
}
```

- [ ] **Step 2: Run — confirm red**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  create_workspace_duplicate_slug_returns_structured_error 2>&1 | tail -10
```

Expected: test panics at `serde_json::from_slice(&body).unwrap()` because the current handler returns an empty body.

- [ ] **Step 3: Add `ErrorBody` to `lib.rs`**

In `signalnode-api/src/lib.rs`, add after the existing `use` block at the top:

```rust
use serde::Serialize;
use std::borrow::Cow;

#[derive(Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: Cow<'static, str>,
}
```

- [ ] **Step 4: Create `workspace/error.rs`**

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum WorkspaceError {
    SlugTaken,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for WorkspaceError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            WorkspaceError::SlugTaken => (
                StatusCode::CONFLICT,
                "slug_taken",
                Cow::Borrowed("A workspace with that slug already exists"),
            ),
            WorkspaceError::InvalidInput(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Cow::Owned(msg))
            }
            WorkspaceError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
```

- [ ] **Step 5: Update `workspace/mod.rs`**

Replace the entire file content:

```rust
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

mod error;
use error::WorkspaceError;

#[derive(Serialize, sqlx::FromRow)]
struct Workspace {
    id: Uuid,
    name: String,
    slug: String,
    owner_id: Uuid,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct CreateWorkspaceRequest {
    name: String,
    slug: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces", post(create_workspace))
        .route("/workspaces", get(list_workspaces))
}

fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() {
        return false;
    }
    let bytes = slug.as_bytes();
    let is_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_alnum(bytes[0]) || !is_alnum(bytes[bytes.len() - 1]) {
        return false;
    }
    slug.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

async fn create_workspace(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<CreateWorkspaceRequest>,
) -> impl IntoResponse {
    if body.name.is_empty() {
        return WorkspaceError::InvalidInput("Name must not be empty".into()).into_response();
    }
    if !is_valid_slug(&body.slug) {
        return WorkspaceError::InvalidInput(
            "Slug must be lowercase alphanumeric, optionally separated by hyphens".into(),
        )
        .into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "failed to begin transaction");
            return WorkspaceError::Internal.into_response();
        }
    };

    let workspace_id = Uuid::new_v4();

    match sqlx::query(
        "INSERT INTO workspaces (id, name, slug, owner_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(&body.name)
    .bind(&body.slug)
    .bind(current_user.id)
    .execute(&mut *tx)
    .await
    {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return WorkspaceError::SlugTaken.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert workspace");
            return WorkspaceError::Internal.into_response();
        }
    }

    if let Err(e) = sqlx::query(
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(current_user.id)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(error = ?e, "failed to insert owner membership");
        return WorkspaceError::Internal.into_response();
    }

    let workspace = match sqlx::query_as::<_, Workspace>(
        "SELECT id, name, slug, owner_id, created_at FROM workspaces WHERE id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = ?e, "failed to fetch created workspace");
            return WorkspaceError::Internal.into_response();
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "failed to commit transaction");
        return WorkspaceError::Internal.into_response();
    }

    (StatusCode::CREATED, Json(workspace)).into_response()
}

async fn list_workspaces(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> impl IntoResponse {
    match sqlx::query_as::<_, Workspace>(
        "SELECT w.id, w.name, w.slug, w.owner_id, w.created_at
         FROM workspaces w
         JOIN workspace_members wm ON wm.workspace_id = w.id
         WHERE wm.user_id = $1
         ORDER BY w.created_at ASC",
    )
    .bind(current_user.id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(ws) => Json(ws).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to list workspaces");
            WorkspaceError::Internal.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use serde_json::json;
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::app;
    use crate::test_helpers::{authed, create_test_user, TEST_JWT_SECRET};

    #[sqlx::test(migrations = "../migrations")]
    async fn create_workspace_success(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let res = authed(
            pool,
            Method::POST,
            "/api/workspaces",
            uid,
            Some(json!({"name": "My Org", "slug": "my-org"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "My Org");
        assert_eq!(json["slug"], "my-org");
        assert_eq!(json["owner_id"], uid.to_string());
        assert!(json["id"].is_string());
        assert!(json["created_at"].is_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_workspace_owner_membership(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        authed(
            pool.clone(),
            Method::POST,
            "/api/workspaces",
            uid,
            Some(json!({"name": "My Org", "slug": "my-org"})),
        )
        .await;
        let row: (Uuid, String) =
            sqlx::query_as("SELECT user_id, role FROM workspace_members WHERE user_id = $1")
                .bind(uid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, uid);
        assert_eq!(row.1, "owner");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_workspace_duplicate_slug(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        authed(
            pool.clone(),
            Method::POST,
            "/api/workspaces",
            uid1,
            Some(json!({"name": "First", "slug": "same-slug"})),
        )
        .await;
        let uid2 = create_test_user(&pool).await;
        let res = authed(
            pool,
            Method::POST,
            "/api/workspaces",
            uid2,
            Some(json!({"name": "Second", "slug": "same-slug"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_workspace_invalid_slug(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        for slug in &["My-Org", "my org", "-leading", "trailing-", "UPPER"] {
            let res = authed(
                pool.clone(),
                Method::POST,
                "/api/workspaces",
                uid,
                Some(json!({"name": "Test", "slug": slug})),
            )
            .await;
            assert_eq!(
                res.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "slug {:?} should be rejected",
                slug
            );
        }
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_workspace_empty_name(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let res = authed(
            pool,
            Method::POST,
            "/api/workspaces",
            uid,
            Some(json!({"name": "", "slug": "valid-slug"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn create_workspace_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/workspaces")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"X","slug":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_workspaces_returns_own(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        authed(
            pool.clone(),
            Method::POST,
            "/api/workspaces",
            uid1,
            Some(json!({"name": "User1 Org", "slug": "user1-org"})),
        )
        .await;
        authed(
            pool.clone(),
            Method::POST,
            "/api/workspaces",
            uid2,
            Some(json!({"name": "User2 Org", "slug": "user2-org"})),
        )
        .await;
        let res = authed(pool, Method::GET, "/api/workspaces", uid1, None).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["slug"], "user1-org");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_workspaces_empty(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let res = authed(pool, Method::GET, "/api/workspaces", uid, None).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_workspaces_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_workspace_duplicate_slug_returns_structured_error(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        authed(
            pool.clone(),
            Method::POST,
            "/api/workspaces",
            uid1,
            Some(json!({"name": "First", "slug": "same-slug"})),
        )
        .await;
        let uid2 = create_test_user(&pool).await;
        let res = authed(
            pool,
            Method::POST,
            "/api/workspaces",
            uid2,
            Some(json!({"name": "Second", "slug": "same-slug"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "slug_taken");
    }
}
```

- [ ] **Step 6: Run — confirm green**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: `test result: ok. 134 passed; 0 failed`

- [ ] **Step 7: Commit**

```bash
git add signalnode-api/src/lib.rs \
        signalnode-api/src/workspace/error.rs \
        signalnode-api/src/workspace/mod.rs
git commit -m "feat(api): structured error bodies for workspace module (Phase 9)"
```

---

## Task 2: Monitor structured errors

**Files:**
- Create: `signalnode-api/src/monitor/error.rs`
- Modify: `signalnode-api/src/monitor/mod.rs`

- [ ] **Step 1: Write the failing body-contract test**

Add at the end of `mod tests` in `signalnode-api/src/monitor/mod.rs`:

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_monitor_empty_name_returns_structured_error(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors"),
        uid,
        Some(json!({"name": "", "url": "https://example.com", "interval_secs": 60})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");
}
```

- [ ] **Step 2: Run — confirm red**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  create_monitor_empty_name_returns_structured_error 2>&1 | tail -10
```

Expected: panics at `from_slice` because body is empty.

- [ ] **Step 3: Create `monitor/error.rs`**

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum MonitorError {
    Forbidden,
    NotFound,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for MonitorError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            MonitorError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            MonitorError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            MonitorError::InvalidInput(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Cow::Owned(msg))
            }
            MonitorError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
```

- [ ] **Step 4: Update `monitor/mod.rs`**

Replace the entire non-test portion. The `use` block, structs, router, helpers, and handlers become:

```rust
use axum::{
    extract::{Path, Query, State},
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

mod error;
use error::MonitorError;

#[derive(Serialize, sqlx::FromRow)]
struct Monitor {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    url: String,
    interval_secs: i32,
    status: String,
    failure_threshold: i32,
    recovery_threshold: i32,
    kind: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct CreateMonitorRequest {
    name: String,
    url: String,
    interval_secs: i32,
    failure_threshold: Option<i32>,
    recovery_threshold: Option<i32>,
}

#[derive(Deserialize, Default)]
struct ListMonitorsQuery {
    include_archived: Option<bool>,
}

#[derive(Deserialize)]
struct PatchMonitorRequest {
    name: Option<String>,
    url: Option<String>,
    interval_secs: Option<i32>,
    status: Option<String>,
    failure_threshold: Option<i32>,
    recovery_threshold: Option<i32>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/{workspace_id}/monitors",
            post(create_monitor).get(list_monitors),
        )
        .route(
            "/workspaces/{workspace_id}/monitors/{monitor_id}",
            get(get_monitor).patch(patch_monitor).delete(delete_monitor),
        )
}

async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), MonitorError> {
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
                Ok(true) => Err(MonitorError::Forbidden),
                Ok(false) => Err(MonitorError::NotFound),
                Err(e) => {
                    tracing::error!(error = ?e, "failed to check workspace existence");
                    Err(MonitorError::Internal)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace membership");
            Err(MonitorError::Internal)
        }
    }
}

async fn check_owner(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), MonitorError> {
    match sqlx::query_scalar::<_, String>(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(role)) if role == "owner" => Ok(()),
        Ok(Some(_)) => Err(MonitorError::Forbidden),
        Ok(None) => match sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)",
        )
        .bind(workspace_id)
        .fetch_one(pool)
        .await
        {
            Ok(true) => Err(MonitorError::Forbidden),
            Ok(false) => Err(MonitorError::NotFound),
            Err(e) => {
                tracing::error!(error = ?e, "failed to check workspace existence");
                Err(MonitorError::Internal)
            }
        },
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace owner");
            Err(MonitorError::Internal)
        }
    }
}

async fn create_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    let failure_threshold = body.failure_threshold.unwrap_or(1);
    let recovery_threshold = body.recovery_threshold.unwrap_or(1);

    if body.name.is_empty()
        || body.url.is_empty()
        || body.interval_secs < 1
        || failure_threshold < 1
        || recovery_threshold < 1
    {
        return MonitorError::InvalidInput(
            "Name and URL must not be empty; interval, failure and recovery thresholds must be >= 1".into(),
        )
        .into_response();
    }

    match sqlx::query_as::<_, Monitor>(
        "INSERT INTO monitors (workspace_id, name, url, interval_secs, failure_threshold, recovery_threshold)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, workspace_id, name, url, interval_secs, status,
                   failure_threshold, recovery_threshold, kind, created_at, updated_at",
    )
    .bind(workspace_id)
    .bind(&body.name)
    .bind(&body.url)
    .bind(body.interval_secs)
    .bind(failure_threshold)
    .bind(recovery_threshold)
    .fetch_one(&state.pool)
    .await
    {
        Ok(monitor) => (StatusCode::CREATED, Json(monitor)).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert monitor");
            MonitorError::Internal.into_response()
        }
    }
}

async fn list_monitors(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<ListMonitorsQuery>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    let sql = if params.include_archived.unwrap_or(false) {
        "SELECT id, workspace_id, name, url, interval_secs, status,
                failure_threshold, recovery_threshold, kind, created_at, updated_at
         FROM monitors WHERE workspace_id = $1 ORDER BY created_at ASC"
    } else {
        "SELECT id, workspace_id, name, url, interval_secs, status,
                failure_threshold, recovery_threshold, kind, created_at, updated_at
         FROM monitors WHERE workspace_id = $1 AND status != 'archived' ORDER BY created_at ASC"
    };

    match sqlx::query_as::<_, Monitor>(sql)
        .bind(workspace_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(monitors) => Json(monitors).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to list monitors");
            MonitorError::Internal.into_response()
        }
    }
}

async fn get_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    match sqlx::query_as::<_, Monitor>(
        "SELECT id, workspace_id, name, url, interval_secs, status,
                failure_threshold, recovery_threshold, kind, created_at, updated_at
         FROM monitors WHERE id = $1 AND workspace_id = $2",
    )
    .bind(monitor_id)
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(monitor)) => Json(monitor).into_response(),
        Ok(None) => MonitorError::NotFound.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to fetch monitor");
            MonitorError::Internal.into_response()
        }
    }
}

async fn patch_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchMonitorRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    if body.name.is_none()
        && body.url.is_none()
        && body.interval_secs.is_none()
        && body.status.is_none()
        && body.failure_threshold.is_none()
        && body.recovery_threshold.is_none()
    {
        return MonitorError::InvalidInput("At least one field must be provided".into())
            .into_response();
    }

    if matches!(&body.name, Some(n) if n.is_empty())
        || matches!(&body.url, Some(u) if u.is_empty())
        || matches!(body.interval_secs, Some(i) if i < 1)
        || matches!(body.failure_threshold, Some(f) if f < 1)
        || matches!(body.recovery_threshold, Some(r) if r < 1)
    {
        return MonitorError::InvalidInput(
            "Name and URL must not be empty; interval, failure and recovery thresholds must be >= 1".into(),
        )
        .into_response();
    }

    if let Some(ref s) = body.status {
        if s != "active" && s != "paused" {
            return MonitorError::InvalidInput(
                "Status must be 'active' or 'paused'".into(),
            )
            .into_response();
        }
    }

    let current_status = match sqlx::query_scalar::<_, String>(
        "SELECT status FROM monitors WHERE id = $1 AND workspace_id = $2",
    )
    .bind(monitor_id)
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(s)) => s,
        Ok(None) => return MonitorError::NotFound.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to fetch monitor status for patch");
            return MonitorError::Internal.into_response();
        }
    };

    if current_status == "archived" {
        return MonitorError::InvalidInput("Archived monitors cannot be modified".into())
            .into_response();
    }

    match sqlx::query_as::<_, Monitor>(
        "UPDATE monitors
         SET name               = COALESCE($1, name),
             url                = COALESCE($2, url),
             interval_secs      = COALESCE($3, interval_secs),
             status             = COALESCE($4, status),
             failure_threshold  = COALESCE($5, failure_threshold),
             recovery_threshold = COALESCE($6, recovery_threshold),
             updated_at         = NOW()
         WHERE id = $7 AND workspace_id = $8
         RETURNING id, workspace_id, name, url, interval_secs, status,
                   failure_threshold, recovery_threshold, kind, created_at, updated_at",
    )
    .bind(body.name)
    .bind(body.url)
    .bind(body.interval_secs)
    .bind(body.status)
    .bind(body.failure_threshold)
    .bind(body.recovery_threshold)
    .bind(monitor_id)
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(monitor)) => Json(monitor).into_response(),
        Ok(None) => MonitorError::NotFound.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to update monitor");
            MonitorError::Internal.into_response()
        }
    }
}

async fn delete_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(e) = check_owner(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    match sqlx::query(
        "UPDATE monitors SET status = 'archived', updated_at = NOW() WHERE id = $1 AND workspace_id = $2",
    )
    .bind(monitor_id)
    .bind(workspace_id)
    .execute(&state.pool)
    .await
    {
        Ok(result) if result.rows_affected() == 0 => MonitorError::NotFound.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to archive monitor");
            MonitorError::Internal.into_response()
        }
    }
}
```

Append the new test to the existing `mod tests` block (all existing tests unchanged):

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_monitor_empty_name_returns_structured_error(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors"),
        uid,
        Some(json!({"name": "", "url": "https://example.com", "interval_secs": 60})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");
}
```

- [ ] **Step 5: Run — confirm green**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: `test result: ok. 135 passed; 0 failed`

- [ ] **Step 6: Commit**

```bash
git add signalnode-api/src/monitor/error.rs \
        signalnode-api/src/monitor/mod.rs
git commit -m "feat(api): structured error bodies for monitor module (Phase 9)"
```

---

## Task 3: Incident structured errors

**Files:**
- Create: `signalnode-api/src/incident/error.rs`
- Modify: `signalnode-api/src/incident/mod.rs`

- [ ] **Step 1: Write the failing body-contract test**

Add at the end of `mod tests` in `signalnode-api/src/incident/mod.rs`:

```rust
#[sqlx::test(migrations = "../migrations")]
async fn list_incidents_forbidden_returns_structured_error(pool: PgPool) {
    let uid1 = create_test_user(&pool).await;
    let uid2 = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid1).await;
    let res = authed(
        pool,
        Method::GET,
        &format!("/api/workspaces/{wid}/incidents"),
        uid2,
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "forbidden");
}
```

- [ ] **Step 2: Run — confirm red**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  list_incidents_forbidden_returns_structured_error 2>&1 | tail -10
```

Expected: panics at `from_slice` because body is empty.

- [ ] **Step 3: Create `incident/error.rs`**

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum IncidentError {
    Forbidden,
    NotFound,
    Internal,
}

impl IntoResponse for IncidentError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            IncidentError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            IncidentError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            IncidentError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
```

- [ ] **Step 4: Update `incident/mod.rs`**

Replace the entire file:

```rust
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

mod error;
use error::IncidentError;

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
) -> Result<(), IncidentError> {
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
                Ok(true) => Err(IncidentError::Forbidden),
                Ok(false) => Err(IncidentError::NotFound),
                Err(e) => {
                    tracing::error!(error = ?e, "failed to check workspace existence");
                    Err(IncidentError::Internal)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace membership");
            Err(IncidentError::Internal)
        }
    }
}

async fn list_open_incidents(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
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
            IncidentError::Internal.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::app;
    use crate::test_helpers::{
        authed, create_test_monitor, create_test_user, create_test_workspace, TEST_JWT_SECRET,
    };

    async fn create_open_incident(pool: &PgPool, monitor_id: Uuid) -> Uuid {
        let incident_id = Uuid::new_v4();
        sqlx::query("INSERT INTO incidents (id, monitor_id) VALUES ($1, $2)")
            .bind(incident_id)
            .bind(monitor_id)
            .execute(pool)
            .await
            .unwrap();
        incident_id
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
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_open_incidents_returns_open_only(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let open_id = create_open_incident(&pool, mid).await;

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
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], open_id.to_string());
        assert!(arr[0]["opened_at"].is_string());
        assert!(arr[0]["monitor_id"].is_string());
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

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid1}/incidents"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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

        sqlx::query("INSERT INTO incidents (id, monitor_id, opened_at) VALUES ($1, $2, NOW())")
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
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
            None,
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
            None,
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

    #[sqlx::test(migrations = "../migrations")]
    async fn list_incidents_forbidden_returns_structured_error(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/incidents"),
            uid2,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "forbidden");
    }
}
```

- [ ] **Step 5: Run — confirm green**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: `test result: ok. 136 passed; 0 failed`

- [ ] **Step 6: Commit**

```bash
git add signalnode-api/src/incident/error.rs \
        signalnode-api/src/incident/mod.rs
git commit -m "feat(api): structured error bodies for incident module (Phase 9)"
```

---

## Task 4: NotificationChannel structured errors

**Files:**
- Create: `signalnode-api/src/notification_channel/error.rs`
- Modify: `signalnode-api/src/notification_channel/mod.rs`

- [ ] **Step 1: Write the failing body-contract test**

Add at the end of `mod tests` in `signalnode-api/src/notification_channel/mod.rs`:

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_channel_empty_target_returns_structured_error(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/notification-channels"),
        uid,
        Some(json!({"kind": "email", "target": ""})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");
}
```

- [ ] **Step 2: Run — confirm red**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  create_channel_empty_target_returns_structured_error 2>&1 | tail -10
```

Expected: panics at `from_slice` because body is empty.

- [ ] **Step 3: Create `notification_channel/error.rs`**

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum NotificationChannelError {
    Forbidden,
    NotFound,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for NotificationChannelError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            NotificationChannelError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            NotificationChannelError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            NotificationChannelError::InvalidInput(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Cow::Owned(msg))
            }
            NotificationChannelError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
```

- [ ] **Step 4: Update `notification_channel/mod.rs` — replace top + handlers**

Replace the non-test portion of the file:

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

mod error;
use error::NotificationChannelError;

#[derive(Serialize, sqlx::FromRow)]
struct NotificationChannel {
    id: Uuid,
    workspace_id: Uuid,
    kind: String,
    target: String,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct CreateChannelRequest {
    kind: String,
    target: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/{workspace_id}/notification-channels",
            post(create_channel).get(list_channels),
        )
        .route(
            "/workspaces/{workspace_id}/notification-channels/{channel_id}",
            delete(delete_channel),
        )
}

async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), NotificationChannelError> {
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
                Ok(true) => Err(NotificationChannelError::Forbidden),
                Ok(false) => Err(NotificationChannelError::NotFound),
                Err(e) => {
                    tracing::error!(error = ?e, "failed to check workspace existence");
                    Err(NotificationChannelError::Internal)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace membership");
            Err(NotificationChannelError::Internal)
        }
    }
}

async fn check_owner(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), NotificationChannelError> {
    match sqlx::query_scalar::<_, String>(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(role)) if role == "owner" => Ok(()),
        Ok(Some(_)) => Err(NotificationChannelError::Forbidden),
        Ok(None) => match sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)",
        )
        .bind(workspace_id)
        .fetch_one(pool)
        .await
        {
            Ok(true) => Err(NotificationChannelError::Forbidden),
            Ok(false) => Err(NotificationChannelError::NotFound),
            Err(e) => {
                tracing::error!(error = ?e, "failed to check workspace existence");
                Err(NotificationChannelError::Internal)
            }
        },
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace owner");
            Err(NotificationChannelError::Internal)
        }
    }
}

async fn create_channel(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateChannelRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_owner(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    if !matches!(body.kind.as_str(), "email" | "webhook") {
        return NotificationChannelError::InvalidInput(
            "Kind must be 'email' or 'webhook'".into(),
        )
        .into_response();
    }
    if body.target.trim().is_empty() {
        return NotificationChannelError::InvalidInput("Target must not be empty".into())
            .into_response();
    }

    match sqlx::query_as::<_, NotificationChannel>(
        "INSERT INTO notification_channels (workspace_id, kind, target) \
         VALUES ($1, $2, $3) \
         RETURNING id, workspace_id, kind, target, created_at",
    )
    .bind(workspace_id)
    .bind(&body.kind)
    .bind(&body.target)
    .fetch_one(&state.pool)
    .await
    {
        Ok(channel) => (StatusCode::CREATED, Json(channel)).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to create notification channel");
            NotificationChannelError::Internal.into_response()
        }
    }
}

async fn list_channels(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    match sqlx::query_as::<_, NotificationChannel>(
        "SELECT id, workspace_id, kind, target, created_at \
         FROM notification_channels \
         WHERE workspace_id = $1 \
         ORDER BY created_at ASC",
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(channels) => Json(channels).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to list notification channels");
            NotificationChannelError::Internal.into_response()
        }
    }
}

async fn delete_channel(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, channel_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(e) = check_owner(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }

    match sqlx::query(
        "DELETE FROM notification_channels WHERE id = $1 AND workspace_id = $2",
    )
    .bind(channel_id)
    .bind(workspace_id)
    .execute(&state.pool)
    .await
    {
        Ok(result) if result.rows_affected() == 0 => {
            NotificationChannelError::NotFound.into_response()
        }
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to delete notification channel");
            NotificationChannelError::Internal.into_response()
        }
    }
}

pub async fn dispatch_notifications(_pool: &PgPool, _incident_id: Uuid) {}
```

The `mod tests` block is unchanged except for appending the new test at the end:

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_channel_empty_target_returns_structured_error(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/notification-channels"),
        uid,
        Some(json!({"kind": "email", "target": ""})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");
}
```

- [ ] **Step 5: Run — confirm green**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: `test result: ok. 137 passed; 0 failed`

- [ ] **Step 6: Commit**

```bash
git add signalnode-api/src/notification_channel/error.rs \
        signalnode-api/src/notification_channel/mod.rs
git commit -m "feat(api): structured error bodies for notification_channel module (Phase 9)"
```

---

## Task 5: CheckResult structured errors

**Files:**
- Create: `signalnode-api/src/check_result/error.rs`
- Modify: `signalnode-api/src/check_result/mod.rs`

- [ ] **Step 1: Write the failing body-contract test**

Add at the end of `mod tests` in `signalnode-api/src/check_result/mod.rs`:

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_check_result_invalid_status_returns_structured_error(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let mid = create_test_monitor(&pool, wid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
        uid,
        Some(json!({"status": "broken"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");
}
```

- [ ] **Step 2: Run — confirm red**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  create_check_result_invalid_status_returns_structured_error 2>&1 | tail -10
```

Expected: panics at `from_slice` because body is empty.

- [ ] **Step 3: Create `check_result/error.rs`**

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum CheckResultError {
    Forbidden,
    NotFound,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for CheckResultError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            CheckResultError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            CheckResultError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            CheckResultError::InvalidInput(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Cow::Owned(msg))
            }
            CheckResultError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
```

- [ ] **Step 4: Update `check_result/mod.rs` — replace helpers and handlers**

Replace the non-test portion:

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

mod error;
use error::CheckResultError;

#[derive(Serialize, sqlx::FromRow)]
struct CheckResult {
    id: Uuid,
    monitor_id: Uuid,
    status: String,
    latency_ms: Option<i32>,
    error_detail: Option<String>,
    checked_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct CreateCheckResultRequest {
    status: String,
    latency_ms: Option<i32>,
    error_detail: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/workspaces/{workspace_id}/monitors/{monitor_id}/check-results",
        post(create_check_result).get(list_check_results),
    )
}

async fn check_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), CheckResultError> {
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
                Ok(true) => Err(CheckResultError::Forbidden),
                Ok(false) => Err(CheckResultError::NotFound),
                Err(e) => {
                    tracing::error!(error = ?e, "failed to check workspace existence");
                    Err(CheckResultError::Internal)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace membership");
            Err(CheckResultError::Internal)
        }
    }
}

async fn resolve_monitor(
    pool: &PgPool,
    workspace_id: Uuid,
    monitor_id: Uuid,
) -> Result<(), CheckResultError> {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM monitors WHERE id = $1 AND workspace_id = $2)",
    )
    .bind(monitor_id)
    .bind(workspace_id)
    .fetch_one(pool)
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(CheckResultError::NotFound),
        Err(e) => {
            tracing::error!(error = ?e, "failed to resolve monitor");
            Err(CheckResultError::Internal)
        }
    }
}

async fn create_check_result(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<CreateCheckResultRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }
    if let Err(e) = resolve_monitor(&state.pool, workspace_id, monitor_id).await {
        return e.into_response();
    }

    if !matches!(body.status.as_str(), "up" | "degraded" | "down") {
        return CheckResultError::InvalidInput(
            "Status must be 'up', 'degraded', or 'down'".into(),
        )
        .into_response();
    }

    let mut opened_incident_id: Option<Uuid> = None;

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "failed to begin transaction");
            return CheckResultError::Internal.into_response();
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
            return CheckResultError::Internal.into_response();
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
                return CheckResultError::Internal.into_response();
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
                return CheckResultError::Internal.into_response();
            }
        };

        if open_incident.is_none() {
            let recent: Vec<String> = match sqlx::query_scalar::<_, String>(
                "SELECT status FROM check_results \
                 WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
            )
            .bind(monitor_id)
            .bind(failure_threshold)
            .fetch_all(&mut *tx)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!(error = ?e, "failed to fetch results for open evaluation");
                    return CheckResultError::Internal.into_response();
                }
            };

            if recent.len() == failure_threshold as usize && recent.iter().all(|s| s == "down") {
                let incident_id = match sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO incidents (monitor_id) VALUES ($1) RETURNING id",
                )
                .bind(monitor_id)
                .fetch_one(&mut *tx)
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!(error = ?e, "failed to open incident");
                        return CheckResultError::Internal.into_response();
                    }
                };

                let channels = match sqlx::query_as::<_, (String, String)>(
                    "SELECT kind, target FROM notification_channels WHERE workspace_id = $1",
                )
                .bind(workspace_id)
                .fetch_all(&mut *tx)
                .await
                {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::error!(error = ?e, "failed to fetch channels for outbox");
                        return CheckResultError::Internal.into_response();
                    }
                };

                for (kind, target) in &channels {
                    if let Err(e) = sqlx::query(
                        "INSERT INTO pending_notifications (incident_id, channel_kind, target) \
                         VALUES ($1, $2, $3)",
                    )
                    .bind(incident_id)
                    .bind(kind)
                    .bind(target)
                    .execute(&mut *tx)
                    .await
                    {
                        tracing::error!(error = ?e, "failed to insert pending notification");
                        return CheckResultError::Internal.into_response();
                    }
                }

                opened_incident_id = Some(incident_id);
            }
        } else {
            let recent: Vec<String> = match sqlx::query_scalar::<_, String>(
                "SELECT status FROM check_results \
                 WHERE monitor_id = $1 ORDER BY checked_at DESC, id DESC LIMIT $2",
            )
            .bind(monitor_id)
            .bind(recovery_threshold)
            .fetch_all(&mut *tx)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!(error = ?e, "failed to fetch results for close evaluation");
                    return CheckResultError::Internal.into_response();
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
                    return CheckResultError::Internal.into_response();
                }
            }
        }
    }

    match tx.commit().await {
        Ok(_) => {
            if let Some(incident_id) = opened_incident_id {
                crate::notification_channel::dispatch_notifications(&state.pool, incident_id)
                    .await;
            }
            (StatusCode::CREATED, Json(cr)).into_response()
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to commit transaction");
            CheckResultError::Internal.into_response()
        }
    }
}

async fn list_check_results(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(e) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return e.into_response();
    }
    if let Err(e) = resolve_monitor(&state.pool, workspace_id, monitor_id).await {
        return e.into_response();
    }

    match sqlx::query_as::<_, CheckResult>(
        "SELECT id, monitor_id, status, latency_ms, error_detail, checked_at
         FROM check_results WHERE monitor_id = $1 ORDER BY checked_at DESC",
    )
    .bind(monitor_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(results) => Json(results).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to list check_results");
            CheckResultError::Internal.into_response()
        }
    }
}
```

The `mod tests` block is unchanged except for appending the new test at the end:

```rust
#[sqlx::test(migrations = "../migrations")]
async fn create_check_result_invalid_status_returns_structured_error(pool: PgPool) {
    let uid = create_test_user(&pool).await;
    let wid = create_test_workspace(&pool, uid).await;
    let mid = create_test_monitor(&pool, wid).await;
    let res = authed(
        pool,
        Method::POST,
        &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
        uid,
        Some(json!({"status": "broken"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "invalid_input");
}
```

- [ ] **Step 5: Run — confirm all green**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: `test result: ok. 138 passed; 0 failed`

Also run core to verify no regressions:

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml 2>&1 | tail -5
```

Expected: `test result: ok. 33 passed; 0 failed`

- [ ] **Step 6: Commit**

```bash
git add signalnode-api/src/check_result/error.rs \
        signalnode-api/src/check_result/mod.rs
git commit -m "feat(api): structured error bodies for check_result module (Phase 9)"
```
