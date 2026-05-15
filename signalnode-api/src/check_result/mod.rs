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

async fn resolve_monitor(
    pool: &PgPool,
    workspace_id: Uuid,
    monitor_id: Uuid,
) -> Result<(), StatusCode> {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM monitors WHERE id = $1 AND workspace_id = $2)",
    )
    .bind(monitor_id)
    .bind(workspace_id)
    .fetch_one(pool)
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(error = ?e, "failed to resolve monitor");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

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

    match sqlx::query_as::<_, CheckResult>(
        "INSERT INTO check_results (monitor_id, status, latency_ms, error_detail)
         VALUES ($1, $2, $3, $4)
         RETURNING id, monitor_id, status, latency_ms, error_detail, checked_at",
    )
    .bind(monitor_id)
    .bind(&body.status)
    .bind(body.latency_ms)
    .bind(body.error_detail.as_deref())
    .fetch_one(&state.pool)
    .await
    {
        Ok(cr) => (StatusCode::CREATED, Json(cr)).into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert check_result");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn list_check_results(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }
    if let Err(status) = resolve_monitor(&state.pool, workspace_id, monitor_id).await {
        return status.into_response();
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
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
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
        app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(req)
            .await
            .unwrap()
    }

    // --- create_check_result tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn create_check_result_success(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(json!({"status": "up", "latency_ms": 42})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "up");
        assert_eq!(json["latency_ms"], 42);
        assert_eq!(json["monitor_id"], mid.to_string());
        assert!(json["id"].is_string());
        assert!(json["checked_at"].is_string());
        assert!(json["error_detail"].is_null());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_check_result_with_error_detail(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            Some(json!({"status": "down", "error_detail": "connection refused"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "down");
        assert_eq!(json["error_detail"], "connection refused");
        assert!(json["latency_ms"].is_null());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_check_result_invalid_status(pool: PgPool) {
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
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_check_result_monitor_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{}/check-results", Uuid::new_v4()),
            uid,
            Some(json!({"status": "up"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_check_result_wrong_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid1 = create_test_workspace(&pool, uid).await;
        let wid2 = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid1).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid2}/monitors/{mid}/check-results"),
            uid,
            Some(json!({"status": "up"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_check_result_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid2,
            Some(json!({"status": "up"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_check_result_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&format!(
                        "/api/workspaces/{}/monitors/{}/check-results",
                        Uuid::new_v4(),
                        Uuid::new_v4()
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"up"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // --- list_check_results tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn list_check_results_empty(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
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
    async fn list_check_results_newest_first(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let id_older = Uuid::new_v4();
        let id_newer = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO check_results (id, monitor_id, status, checked_at) VALUES ($1, $2, 'up', NOW() - INTERVAL '10 seconds')",
        )
        .bind(id_older)
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO check_results (id, monitor_id, status, checked_at) VALUES ($1, $2, 'down', NOW())",
        )
        .bind(id_newer)
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], id_newer.to_string());
        assert_eq!(arr[1]["id"], id_older.to_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_check_results_scoped_to_monitor(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid1 = create_test_monitor(&pool, wid).await;
        let mid2 = create_test_monitor(&pool, wid).await;

        sqlx::query(
            "INSERT INTO check_results (monitor_id, status) VALUES ($1, 'up')",
        )
        .bind(mid2)
        .execute(&pool)
        .await
        .unwrap();

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors/{mid1}/check-results"),
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
    async fn list_check_results_monitor_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!(
                "/api/workspaces/{wid}/monitors/{}/check-results",
                Uuid::new_v4()
            ),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_check_results_wrong_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid1 = create_test_workspace(&pool, uid).await;
        let wid2 = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid1).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid2}/monitors/{mid}/check-results"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_check_results_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors/{mid}/check-results"),
            uid2,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn list_check_results_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(&format!(
                        "/api/workspaces/{}/monitors/{}/check-results",
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
}
