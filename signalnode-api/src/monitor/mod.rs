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

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/{workspace_id}/monitors",
            post(create_monitor).get(list_monitors),
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

async fn create_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    let failure_threshold = body.failure_threshold.unwrap_or(1);
    let recovery_threshold = body.recovery_threshold.unwrap_or(1);

    if body.name.is_empty()
        || body.url.is_empty()
        || body.interval_secs < 1
        || failure_threshold < 1
        || recovery_threshold < 1
    {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
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
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn list_monitors(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<ListMonitorsQuery>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
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
            json!({"name": "Test", "url": "https://example.com", "interval_secs": -1}),
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

    #[sqlx::test(migrations = "../migrations")]
    async fn create_monitor_includes_new_fields(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            Some(json!({"name": "M", "url": "https://example.com", "interval_secs": 60})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "active");
        assert_eq!(json["kind"], "uptime");
        assert_eq!(json["failure_threshold"], 1);
        assert_eq!(json["recovery_threshold"], 1);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_monitor_explicit_thresholds(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let res = authed(
            pool,
            Method::POST,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            Some(json!({
                "name": "M",
                "url": "https://example.com",
                "interval_secs": 60,
                "failure_threshold": 3,
                "recovery_threshold": 2
            })),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["failure_threshold"], 3);
        assert_eq!(json["recovery_threshold"], 2);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn create_monitor_invalid_threshold(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        for body in &[
            json!({"name": "M", "url": "https://example.com", "interval_secs": 60, "failure_threshold": 0}),
            json!({"name": "M", "url": "https://example.com", "interval_secs": 60, "recovery_threshold": 0}),
        ] {
            let res = authed(
                pool.clone(),
                Method::POST,
                &format!("/api/workspaces/{wid}/monitors"),
                uid,
                Some(body.clone()),
            )
            .await;
            assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "body {body:?} should be 422");
        }
    }

    // --- archived filter helpers ---

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

    async fn archive_monitor(pool: &PgPool, monitor_id: Uuid) {
        sqlx::query("UPDATE monitors SET status = 'archived' WHERE id = $1")
            .bind(monitor_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_monitors_excludes_archived_by_default(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;
        archive_monitor(&pool, mid).await;

        let res = authed(pool, Method::GET, &format!("/api/workspaces/{wid}/monitors"), uid, None).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0, "archived monitor must not appear in default list");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn list_monitors_include_archived_flag(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;
        archive_monitor(&pool, mid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors?include_archived=true"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1, "archived monitor must appear with include_archived=true");
        assert_eq!(json[0]["status"], "archived");
    }
}
