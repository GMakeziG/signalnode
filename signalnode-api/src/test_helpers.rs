use axum::body::Body;
use axum::http::{header, Method, Request};
use sqlx::PgPool;
use tower::ServiceExt;
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
    sqlx::query("INSERT INTO workspaces (id, name, slug, owner_id) VALUES ($1, $2, $3, $4)")
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
