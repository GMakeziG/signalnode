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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use serde_json::json;
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::app;
    use crate::test_helpers::{authed, create_test_user, create_test_workspace, TEST_JWT_SECRET};

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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
            &format!(
                "/api/workspaces/{wid}/notification-channels/{}",
                Uuid::new_v4()
            ),
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
}
