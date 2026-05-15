# NotificationChannel + Outbox Stub — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add NotificationChannel CRUD (POST/GET/DELETE), a transactional outbox (`pending_notifications`), and a stub dispatch call after an Incident opens — proving the full notification path without real delivery.

**Architecture:** Four commits in order: (1) extract shared test helpers into `src/test_helpers.rs`; (2) add two migrations; (3) add the `notification_channel` module with all three routes and a stub `dispatch_notifications` function; (4) wire the outbox fanout into `create_check_result`. The stub logs queued notifications after commit — no worker, no real delivery.

**Tech Stack:** Rust, Axum, sqlx (Postgres), `#[sqlx::test]` for integration tests, `tracing` for structured logging.

**Spec:** `docs/superpowers/specs/2026-05-15-notification-channel-design.md`

---

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Create | `signalnode-api/src/test_helpers.rs` | Shared test helpers used by all modules |
| Modify | `signalnode-api/src/lib.rs` | Add `pub mod notification_channel`; add `#[cfg(test)] pub mod test_helpers`; wire router |
| Modify | `signalnode-api/src/workspace/mod.rs` | Remove duplicated test helpers; use shared ones |
| Modify | `signalnode-api/src/monitor/mod.rs` | Remove duplicated test helpers; use shared ones |
| Modify | `signalnode-api/src/check_result/mod.rs` | Remove duplicated test helpers; use shared ones; add outbox fanout + 3 outbox tests |
| Modify | `signalnode-api/src/incident/mod.rs` | Remove duplicated test helpers; update `authed` call sites (add `None` body arg) |
| Create | `migrations/20260515000008_notification_channels.sql` | `notification_channels` table |
| Create | `migrations/20260515000009_pending_notifications.sql` | `pending_notifications` table |
| Create | `signalnode-api/src/notification_channel/mod.rs` | All routes + stub dispatch function + 17 tests |

---

## Task 1: Extract shared test helpers

**Files:**
- Create: `signalnode-api/src/test_helpers.rs`
- Modify: `signalnode-api/src/lib.rs`
- Modify: `signalnode-api/src/workspace/mod.rs`
- Modify: `signalnode-api/src/monitor/mod.rs`
- Modify: `signalnode-api/src/check_result/mod.rs`
- Modify: `signalnode-api/src/incident/mod.rs`

- [ ] **Step 1: Create `signalnode-api/src/test_helpers.rs`**

```rust
use axum::body::Body;
use axum::http::{header, Method, Request};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{app, auth::token::encode_access_token};

pub const TEST_JWT_SECRET: &str = "test-secret-at-least-32-chars-long!";

pub async fn create_test_user(pool: &PgPool) -> Uuid {
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

pub async fn create_test_workspace(pool: &PgPool, user_id: Uuid) -> Uuid {
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

pub async fn create_test_monitor(pool: &PgPool, workspace_id: Uuid) -> Uuid {
    let monitor_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO monitors (id, workspace_id, name, url, interval_secs) \
         VALUES ($1, $2, $3, $4, $5)",
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

pub async fn create_test_monitor_thresholds(
    pool: &PgPool,
    workspace_id: Uuid,
    failure_threshold: i32,
    recovery_threshold: i32,
) -> Uuid {
    let monitor_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO monitors \
         (id, workspace_id, name, url, interval_secs, failure_threshold, recovery_threshold) \
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

pub async fn authed(
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
    app(pool, TEST_JWT_SECRET.to_string())
        .oneshot(req)
        .await
        .unwrap()
}
```

- [ ] **Step 2: Register `test_helpers` in `lib.rs`**

Add after the existing `pub mod workspace;` line in `signalnode-api/src/lib.rs`:

```rust
#[cfg(test)]
pub mod test_helpers;
```

- [ ] **Step 3: Update `workspace/mod.rs` tests**

Inside the `#[cfg(test)] mod tests { ... }` block:

Replace the existing imports block that includes `use crate::{app, auth::token::encode_access_token};`:

```rust
use axum::http::{Method, StatusCode};
use axum::body::Body;
use axum::http::Request;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::test_helpers::{authed, create_test_user, TEST_JWT_SECRET};
use crate::app;
use crate::auth::token::encode_access_token;
```

Remove the local `TEST_JWT_SECRET` constant, `create_test_user` function, and `authed` function definitions from `workspace/mod.rs` — they are now in `test_helpers`.

- [ ] **Step 4: Update `monitor/mod.rs` tests**

Inside the `#[cfg(test)] mod tests { ... }` block, replace the imports and remove the duplicated helpers. Add:

```rust
use crate::test_helpers::{
    authed, create_test_user, create_test_workspace, create_test_monitor,
    create_test_monitor_thresholds, TEST_JWT_SECRET,
};
```

Remove the local definitions of `TEST_JWT_SECRET`, `create_test_user`, `create_test_workspace`, `create_test_monitor`, and `authed` from `monitor/mod.rs`. Keep `create_test_member` — it is monitor-specific (inserts a member-role row) and not shared.

- [ ] **Step 5: Update `check_result/mod.rs` tests**

Add to the imports inside `mod tests`:

```rust
use crate::test_helpers::{
    authed, create_test_user, create_test_workspace, create_test_monitor,
    create_test_monitor_thresholds, TEST_JWT_SECRET,
};
```

Remove the local definitions of `TEST_JWT_SECRET`, `create_test_user`, `create_test_workspace`, `create_test_monitor`, `create_test_monitor_thresholds`, and `authed`. Keep `open_incident_count` and `closed_incident_count` — they are check-result-specific.

- [ ] **Step 6: Update `incident/mod.rs` tests**

The incident module's local `authed` does NOT take a body argument. After switching to the shared version, every call site must pass `None` as the fifth argument.

Add to the imports inside `mod tests`:

```rust
use crate::test_helpers::{
    authed, create_test_user, create_test_workspace, create_test_monitor, TEST_JWT_SECRET,
};
```

Remove the local definitions of `TEST_JWT_SECRET`, `create_test_user`, `create_test_workspace`, `create_test_monitor`, and `authed`.

Update every `authed(...)` call in `incident/mod.rs` — they currently have four arguments; add `None` as the fifth:

```rust
// Before
authed(pool, Method::GET, &format!("/api/workspaces/{wid}/incidents"), uid).await

// After
authed(pool, Method::GET, &format!("/api/workspaces/{wid}/incidents"), uid, None).await
```

There are seven such calls — one per test function that calls `authed`. Keep `create_open_incident` — it is incident-specific.

- [ ] **Step 7: Run the full test suite and confirm no regressions**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: all 96 tests pass, zero failures.

- [ ] **Step 8: Commit**

```bash
git add signalnode-api/src/test_helpers.rs \
        signalnode-api/src/lib.rs \
        signalnode-api/src/workspace/mod.rs \
        signalnode-api/src/monitor/mod.rs \
        signalnode-api/src/check_result/mod.rs \
        signalnode-api/src/incident/mod.rs
git commit -m "refactor: extract shared test helpers into test_helpers module"
```

---

## Task 2: Add migrations

**Files:**
- Create: `migrations/20260515000008_notification_channels.sql`
- Create: `migrations/20260515000009_pending_notifications.sql`

- [ ] **Step 1: Create `migrations/20260515000008_notification_channels.sql`**

```sql
CREATE TABLE notification_channels (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind         TEXT        NOT NULL CHECK (kind IN ('email', 'webhook')),
    target       TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX notification_channels_workspace_id_idx
    ON notification_channels (workspace_id);
```

- [ ] **Step 2: Create `migrations/20260515000009_pending_notifications.sql`**

```sql
CREATE TABLE pending_notifications (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    incident_id  UUID        NOT NULL REFERENCES incidents(id) ON DELETE CASCADE,
    channel_kind TEXT        NOT NULL CHECK (channel_kind IN ('email', 'webhook')),
    target       TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX pending_notifications_incident_id_idx
    ON pending_notifications (incident_id);
CREATE INDEX pending_notifications_created_at_idx
    ON pending_notifications (created_at);
```

- [ ] **Step 3: Verify migrations apply cleanly**

`sqlx::test` automatically runs all migrations for every integration test. Running the suite proves both migrations apply without error:

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: still 96 tests pass.

- [ ] **Step 4: Commit**

```bash
git add migrations/20260515000008_notification_channels.sql \
        migrations/20260515000009_pending_notifications.sql
git commit -m "feat: add notification_channels and pending_notifications migrations"
```

---

## Task 3: NotificationChannel CRUD routes

**Files:**
- Create: `signalnode-api/src/notification_channel/mod.rs`
- Modify: `signalnode-api/src/lib.rs`

### Step 1-2: Skeleton and registration

- [ ] **Step 1: Create the module skeleton**

Create `signalnode-api/src/notification_channel/mod.rs` with stub handlers that compile but return `500` (tests will fail for the right reason — wrong status — not compile errors):

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

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

async fn check_owner(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode> {
    match sqlx::query_scalar::<_, String>(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(role)) if role == "owner" => Ok(()),
        Ok(Some(_)) => Err(StatusCode::FORBIDDEN),
        Ok(None) => match sqlx::query_scalar::<_, bool>(
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
        },
        Err(e) => {
            tracing::error!(error = ?e, "failed to check workspace owner");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn create_channel(
    State(_state): State<AppState>,
    _current_user: CurrentUser,
    Path(_workspace_id): Path<Uuid>,
    Json(_body): Json<CreateChannelRequest>,
) -> impl IntoResponse {
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn list_channels(
    State(_state): State<AppState>,
    _current_user: CurrentUser,
    Path(_workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn delete_channel(
    State(_state): State<AppState>,
    _current_user: CurrentUser,
    Path((_workspace_id, _channel_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    StatusCode::INTERNAL_SERVER_ERROR
}

pub async fn dispatch_notifications(pool: &PgPool, incident_id: Uuid) {
    let _ = (pool, incident_id);
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};
    use sqlx::PgPool;
    use uuid::Uuid;

    use crate::test_helpers::{
        authed, create_test_user, create_test_workspace, TEST_JWT_SECRET,
    };
    use crate::app;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    use serde_json::json;
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `signalnode-api/src/lib.rs`, add the module declaration after `pub mod monitor;`:

```rust
pub mod notification_channel;
```

Add the router to the `protected` block in the `app` function, after `.nest("/api", incident::router())`:

```rust
.nest("/api", notification_channel::router())
```

Run `cargo check` to confirm it compiles:

```bash
cargo check --manifest-path signalnode-api/Cargo.toml
```

Expected: no errors.

### POST route — TDD

- [ ] **Step 3: Write the POST tests (failing)**

Add these tests inside `mod tests` in `notification_channel/mod.rs`:

```rust
    async fn create_test_channel(pool: &PgPool, workspace_id: Uuid) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO notification_channels (workspace_id, kind, target) \
             VALUES ($1, 'webhook', 'https://example.com/hook') RETURNING id",
        )
        .bind(workspace_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_channel_success(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/notification-channels"),
            uid,
            Some(json!({"kind": "webhook", "target": "https://hooks.example.com/abc"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["id"].is_string());
        assert_eq!(json["workspace_id"], wid.to_string());
        assert_eq!(json["kind"], "webhook");
        assert_eq!(json["target"], "https://hooks.example.com/abc");
        assert!(json["created_at"].is_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_channel_invalid_kind(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/notification-channels"),
            uid,
            Some(json!({"kind": "sms", "target": "555-1234"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_channel_empty_target(pool: PgPool) {
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
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_channel_not_member(pool: PgPool) {
        let uid = create_test_user(&pool).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{}/notification-channels", Uuid::new_v4()),
            uid,
            Some(json!({"kind": "webhook", "target": "https://example.com"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_channel_member_not_owner(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let member = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(wid)
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/notification-channels"),
            member,
            Some(json!({"kind": "webhook", "target": "https://example.com"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_channel_unauthenticated() {
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&format!(
                        "/api/workspaces/{}/notification-channels",
                        Uuid::new_v4()
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"kind":"webhook","target":"https://x.com"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
```

- [ ] **Step 4: Run POST tests and confirm they fail for the right reason**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  notification_channel::tests::create_channel 2>&1 | grep -E "FAILED|PASSED|panicked"
```

Expected: `create_channel_success` fails with status mismatch (got 500, expected 201). `create_channel_unauthenticated` passes (auth middleware fires before handler).

- [ ] **Step 5: Implement `create_channel`**

Replace the stub `create_channel` in `notification_channel/mod.rs`:

```rust
async fn create_channel(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateChannelRequest>,
) -> impl IntoResponse {
    if let Err(status) = check_owner(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    if !matches!(body.kind.as_str(), "email" | "webhook") {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    if body.target.trim().is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
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
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

- [ ] **Step 6: Run POST tests and confirm they pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  notification_channel::tests::create_channel 2>&1 | grep -E "FAILED|ok"
```

Expected: all 6 POST tests pass.

### GET route — TDD

- [ ] **Step 7: Write the GET tests (failing)**

Add to `mod tests`:

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn list_channels_empty(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/notification-channels"),
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
    async fn list_channels_member_can_read(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let member = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(wid)
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
        create_test_channel(&pool, wid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/notification-channels"),
            member,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_channels_ordered_oldest_first(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let older_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO notification_channels (workspace_id, kind, target, created_at) \
             VALUES ($1, 'webhook', 'https://a.example.com', NOW() - INTERVAL '10 seconds') \
             RETURNING id",
        )
        .bind(wid)
        .fetch_one(&pool)
        .await
        .unwrap();

        let newer_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO notification_channels (workspace_id, kind, target, created_at) \
             VALUES ($1, 'email', 'b@example.com', NOW()) RETURNING id",
        )
        .bind(wid)
        .fetch_one(&pool)
        .await
        .unwrap();

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/notification-channels"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], older_id.to_string());
        assert_eq!(arr[1]["id"], newer_id.to_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_channels_scoped_to_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid1 = create_test_workspace(&pool, uid).await;
        let wid2 = create_test_workspace(&pool, uid).await;
        create_test_channel(&pool, wid1).await;
        create_test_channel(&pool, wid2).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid1}/notification-channels"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_channels_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/notification-channels"),
            uid2,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn list_channels_unauthenticated() {
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(&format!(
                        "/api/workspaces/{}/notification-channels",
                        Uuid::new_v4()
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
```

- [ ] **Step 8: Run GET tests and confirm they fail for the right reason**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  notification_channel::tests::list_channels 2>&1 | grep -E "FAILED|ok"
```

Expected: DB-backed tests fail (handler returns 500); `list_channels_unauthenticated` passes.

- [ ] **Step 9: Implement `list_channels`**

Replace the stub `list_channels`:

```rust
async fn list_channels(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
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
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

- [ ] **Step 10: Run GET tests and confirm they pass**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  notification_channel::tests::list_channels 2>&1 | grep -E "FAILED|ok"
```

Expected: all 6 GET tests pass.

### DELETE route — TDD

- [ ] **Step 11: Write the DELETE tests (failing)**

Add to `mod tests`:

```rust
    #[sqlx::test(migrations = "../migrations")]
    async fn delete_channel_success(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let cid = create_test_channel(&pool, wid).await;

        let res = authed(
            pool.clone(),
            Method::DELETE,
            &format!("/api/workspaces/{wid}/notification-channels/{cid}"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM notification_channels WHERE id = $1",
        )
        .bind(cid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_channel_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::DELETE,
            &format!("/api/workspaces/{wid}/notification-channels/{}", Uuid::new_v4()),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_channel_wrong_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid1 = create_test_workspace(&pool, uid).await;
        let wid2 = create_test_workspace(&pool, uid).await;
        let cid = create_test_channel(&pool, wid1).await;

        let res = authed(
            pool,
            Method::DELETE,
            &format!("/api/workspaces/{wid2}/notification-channels/{cid}"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_channel_member_not_owner(pool: PgPool) {
        let owner = create_test_user(&pool).await;
        let member = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, owner).await;
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(wid)
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
        let cid = create_test_channel(&pool, wid).await;

        let res = authed(
            pool,
            Method::DELETE,
            &format!("/api/workspaces/{wid}/notification-channels/{cid}"),
            member,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_channel_unauthenticated() {
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(&format!(
                        "/api/workspaces/{}/notification-channels/{}",
                        Uuid::new_v4(),
                        Uuid::new_v4()
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
```

- [ ] **Step 12: Run DELETE tests and confirm they fail for the right reason**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  notification_channel::tests::delete_channel 2>&1 | grep -E "FAILED|ok"
```

Expected: DB-backed tests fail; `delete_channel_unauthenticated` passes.

- [ ] **Step 13: Implement `delete_channel` and the `dispatch_notifications` stub**

Replace the stub `delete_channel`:

```rust
async fn delete_channel(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, channel_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(status) = check_owner(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    match sqlx::query(
        "DELETE FROM notification_channels WHERE id = $1 AND workspace_id = $2",
    )
    .bind(channel_id)
    .bind(workspace_id)
    .execute(&state.pool)
    .await
    {
        Ok(result) if result.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to delete notification channel");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

Replace the stub `dispatch_notifications`:

```rust
pub async fn dispatch_notifications(pool: &PgPool, incident_id: Uuid) {
    let rows = match sqlx::query_as::<_, (String, String)>(
        "SELECT channel_kind, target FROM pending_notifications WHERE incident_id = $1",
    )
    .bind(incident_id)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = ?e, %incident_id, "failed to fetch pending notifications");
            return;
        }
    };

    for (channel_kind, target) in rows {
        tracing::info!(channel_kind, target, %incident_id, "stub: notification queued");
    }
}
```

- [ ] **Step 14: Run all notification_channel tests**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  notification_channel 2>&1 | grep -E "FAILED|ok|test result"
```

Expected: all 17 tests pass.

- [ ] **Step 15: Run the full suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: 113 tests pass (96 + 17), zero failures.

- [ ] **Step 16: Commit**

```bash
git add signalnode-api/src/notification_channel/mod.rs \
        signalnode-api/src/lib.rs
git commit -m "feat: add NotificationChannel CRUD and dispatch_notifications stub"
```

---

## Task 4: Outbox fanout in `create_check_result`

**Files:**
- Modify: `signalnode-api/src/check_result/mod.rs`

- [ ] **Step 1: Write the 3 outbox integration tests (failing)**

Add these to the `mod tests` block in `check_result/mod.rs`, after the existing incident close tests:

```rust
    // --- outbox fanout tests ---

    async fn pending_notification_count(pool: &PgPool, incident_id: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pending_notifications WHERE incident_id = $1",
        )
        .bind(incident_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn open_incident_id(pool: &PgPool, monitor_id: Uuid) -> Option<Uuid> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM incidents WHERE monitor_id = $1 AND closed_at IS NULL LIMIT 1",
        )
        .bind(monitor_id)
        .fetch_optional(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn pending_notifications_created_when_incident_opens(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 1: one down result opens an incident
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 1).await;

        // Create one notification channel so there is something to fan out to
        sqlx::query(
            "INSERT INTO notification_channels (workspace_id, kind, target) \
             VALUES ($1, 'webhook', 'https://hooks.example.com/test')",
        )
        .bind(wid)
        .execute(&pool)
        .await
        .unwrap();

        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(json!({"status": "down"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);

        let iid = open_incident_id(&pool, mid).await.expect("incident should be open");
        assert_eq!(pending_notification_count(&pool, iid).await, 1);

        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT channel_kind, target FROM pending_notifications WHERE incident_id = $1",
        )
        .bind(iid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "webhook");
        assert_eq!(row.1, "https://hooks.example.com/test");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn no_pending_notifications_when_incident_does_not_open(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 1: an "up" result does not open an incident
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 1).await;

        sqlx::query(
            "INSERT INTO notification_channels (workspace_id, kind, target) \
             VALUES ($1, 'email', 'alert@example.com')",
        )
        .bind(wid)
        .execute(&pool)
        .await
        .unwrap();

        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(json!({"status": "up"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);

        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pending_notifications",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn no_pending_notifications_when_no_channels(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 1: one down result opens an incident
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 1).await;
        // No channels registered for this workspace

        let res = authed(
            pool.clone(),
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(json!({"status": "down"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);

        let iid = open_incident_id(&pool, mid).await.expect("incident should be open");
        assert_eq!(pending_notification_count(&pool, iid).await, 0);
    }
```

- [ ] **Step 2: Run outbox tests and confirm they fail for the right reason**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml \
  check_result::tests::pending_notifications 2>&1 | grep -E "FAILED|ok"
```

Expected: all three fail — `pending_notifications_created_when_incident_opens` fails because `pending_notifications` count is 0 (not 1). The incident still opens correctly, just no fanout yet.

- [ ] **Step 3: Wire outbox fanout into `create_check_result`**

In `signalnode-api/src/check_result/mod.rs`, make three changes:

**Change 1** — add `opened_incident_id` binding before the transaction begins. Find the line `let mut tx = match state.pool.begin()...` and add immediately before it:

```rust
    let mut opened_incident_id: Option<Uuid> = None;
```

**Change 2** — inside the open-incident block, replace the existing `sqlx::query("INSERT INTO incidents ...")` block with the following (captures the id, fans out to channels, records in outbox):

```rust
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
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }

            opened_incident_id = Some(incident_id);
```

**Change 3** — replace the `tx.commit()` match block at the end of the function:

```rust
    match tx.commit().await {
        Ok(_) => {
            if let Some(incident_id) = opened_incident_id {
                crate::notification_channel::dispatch_notifications(
                    &state.pool,
                    incident_id,
                )
                .await;
            }
            (StatusCode::CREATED, Json(cr)).into_response()
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to commit transaction");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
```

- [ ] **Step 4: Run the full suite**

```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml 2>&1 | tail -5
```

Expected: 116 tests pass (96 + 17 + 3), zero failures.

- [ ] **Step 5: Commit**

```bash
git add signalnode-api/src/check_result/mod.rs
git commit -m "feat: wire outbox fanout into create_check_result; call dispatch_notifications stub post-commit"
```
