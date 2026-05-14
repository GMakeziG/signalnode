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
