# Workspace + Membership Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add workspaces and workspace_members tables, two protected Axum routes (create, list), and 9 DB-backed + unit tests.

**Architecture:** Two SQL migrations define the schema. A new `workspace` module mirrors the `auth` module pattern — handlers and tests in one file, a `router()` function returning `Router<AppState>`. The workspace router is nested under `/api` inside the existing protected block in `lib.rs`, so all workspace routes inherit the JWT middleware automatically.

**Tech Stack:** Rust, Axum 0.8, sqlx 0.8 (Postgres + `FromRow` + `chrono`), uuid, tracing

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `migrations/20260514000002_workspaces.sql` | workspaces table DDL |
| Create | `migrations/20260514000003_workspace_members.sql` | workspace_members table DDL |
| Create | `signalnode-api/src/workspace/mod.rs` | types, validation, handlers, tests |
| Modify | `signalnode-api/src/lib.rs` | declare `pub mod workspace`, nest workspace router |

---

### Task 1: Add migrations

**Files:**
- Create: `migrations/20260514000002_workspaces.sql`
- Create: `migrations/20260514000003_workspace_members.sql`

- [ ] **Step 1: Create workspaces migration**

`migrations/20260514000002_workspaces.sql`:
```sql
CREATE TABLE workspaces (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT        NOT NULL,
    slug       TEXT        NOT NULL UNIQUE,
    owner_id   UUID        NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

- [ ] **Step 2: Create workspace_members migration**

`migrations/20260514000003_workspace_members.sql`:
```sql
CREATE TABLE workspace_members (
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id      UUID        NOT NULL REFERENCES users(id)      ON DELETE CASCADE,
    role         TEXT        NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'member')),
    joined_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (workspace_id, user_id)
);
```

- [ ] **Step 3: Verify files exist**

```bash
ls migrations/
```
Expected: four files — `20260513000000_initial.sql`, `20260513000001_users.sql`, `20260514000002_workspaces.sql`, `20260514000003_workspace_members.sql`

- [ ] **Step 4: Commit migrations**

```bash
git -C /home/ninjatronics/src/signalnode add migrations/20260514000002_workspaces.sql migrations/20260514000003_workspace_members.sql
git -C /home/ninjatronics/src/signalnode commit -m "feat: add workspaces and workspace_members migrations"
```

---

### Task 2: Scaffold workspace module with all tests (RED)

**Files:**
- Create: `signalnode-api/src/workspace/mod.rs`
- Modify: `signalnode-api/src/lib.rs`

Stub handlers return `StatusCode::NOT_IMPLEMENTED`. All 9 tests are written now. After wiring, unit tests (no DB) pass immediately; DB-backed tests fail against stubs.

- [ ] **Step 1: Create `signalnode-api/src/workspace/mod.rs`**

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
    slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

async fn create_workspace(
    State(_state): State<AppState>,
    _current_user: CurrentUser,
    Json(_body): Json<CreateWorkspaceRequest>,
) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn list_workspaces(
    State(_state): State<AppState>,
    _current_user: CurrentUser,
) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

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

    // --- create_workspace tests ---

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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
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
        let row: (Uuid, String) = sqlx::query_as(
            "SELECT user_id, role FROM workspace_members WHERE user_id = $1",
        )
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

    // --- list_workspaces tests ---

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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
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
}
```

- [ ] **Step 2: Wire into `signalnode-api/src/lib.rs`**

Add `pub mod workspace;` after the existing module declarations:
```rust
pub mod auth;
pub mod middleware;
pub mod workspace;
```

Nest workspace router into the protected block (before `route_layer`):
```rust
let protected = Router::new()
    .route("/api/me", get(me))
    .nest("/api", workspace::router())
    .route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        middleware::auth_middleware,
    ));
```

- [ ] **Step 3: Verify compilation**

```bash
cd /home/ninjatronics/src/signalnode/signalnode-api && cargo build 2>&1
```
Expected: compiles with no errors. Warnings about unused parameters on stubs are fine.

- [ ] **Step 4: Run workspace tests — verify RED**

```bash
cd /home/ninjatronics/src/signalnode/signalnode-api && cargo test workspace 2>&1
```
Expected:
- `create_workspace_unauthenticated` → PASS (middleware rejects before the stub)
- `list_workspaces_unauthenticated` → PASS (same reason)
- All DB-backed tests → FAIL (stubs return 501, tests expect 201/200/409/422)

---

### Task 3: Implement `create_workspace`

**Files:**
- Modify: `signalnode-api/src/workspace/mod.rs` — replace `create_workspace` stub

- [ ] **Step 1: Replace the stub**

Replace the entire `create_workspace` function:

```rust
async fn create_workspace(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<CreateWorkspaceRequest>,
) -> impl IntoResponse {
    if body.name.is_empty() || !is_valid_slug(&body.slug) {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "failed to begin transaction");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
            return StatusCode::CONFLICT.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert workspace");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "failed to commit transaction");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (StatusCode::CREATED, Json(workspace)).into_response()
}
```

- [ ] **Step 2: Run create_workspace tests — verify GREEN**

```bash
cd /home/ninjatronics/src/signalnode/signalnode-api && cargo test workspace::tests::create_workspace 2>&1
```
Expected: 6 tests pass — `create_workspace_success`, `create_workspace_owner_membership`, `create_workspace_duplicate_slug`, `create_workspace_invalid_slug`, `create_workspace_empty_name`, `create_workspace_unauthenticated`.

---

### Task 4: Implement `list_workspaces` and verify full suite

**Files:**
- Modify: `signalnode-api/src/workspace/mod.rs` — replace `list_workspaces` stub

- [ ] **Step 1: Replace the stub**

Replace the entire `list_workspaces` function:

```rust
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
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

- [ ] **Step 2: Run all workspace tests — verify GREEN**

```bash
cd /home/ninjatronics/src/signalnode/signalnode-api && cargo test workspace 2>&1
```
Expected: all 9 workspace tests pass.

- [ ] **Step 3: Run full test suite — verify no regressions**

```bash
cd /home/ninjatronics/src/signalnode/signalnode-api && cargo test 2>&1
```
Expected: all tests pass (workspace + auth + token).

- [ ] **Step 4: Commit**

```bash
git -C /home/ninjatronics/src/signalnode add signalnode-api/src/workspace/mod.rs signalnode-api/src/lib.rs
git -C /home/ninjatronics/src/signalnode commit -m "feat: add workspace + membership foundation (create, list, tests)"
```
