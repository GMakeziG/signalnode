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

async fn check_owner(pool: &PgPool, workspace_id: Uuid, user_id: Uuid) -> Result<(), StatusCode> {
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

async fn get_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
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
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to fetch monitor");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn patch_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchMonitorRequest>,
) -> impl IntoResponse {
    if let Err(status) = check_membership(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    if body.name.is_none()
        && body.url.is_none()
        && body.interval_secs.is_none()
        && body.status.is_none()
        && body.failure_threshold.is_none()
        && body.recovery_threshold.is_none()
    {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    if matches!(&body.name, Some(n) if n.is_empty())
        || matches!(&body.url, Some(u) if u.is_empty())
        || matches!(body.interval_secs, Some(i) if i < 1)
        || matches!(body.failure_threshold, Some(f) if f < 1)
        || matches!(body.recovery_threshold, Some(r) if r < 1)
    {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    if let Some(ref s) = body.status {
        if s != "active" && s != "paused" {
            return StatusCode::UNPROCESSABLE_ENTITY.into_response();
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
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to fetch monitor status for patch");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if current_status == "archived" {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
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
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to update monitor");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn delete_monitor(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path((workspace_id, monitor_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(status) = check_owner(&state.pool, workspace_id, current_user.id).await {
        return status.into_response();
    }

    match sqlx::query(
        "UPDATE monitors SET status = 'archived', updated_at = NOW() WHERE id = $1 AND workspace_id = $2",
    )
    .bind(monitor_id)
    .bind(workspace_id)
    .execute(&state.pool)
    .await
    {
        Ok(result) if result.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to archive monitor");
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

    use crate::app;
    use crate::test_helpers::{
        authed, create_test_monitor, create_test_user, create_test_workspace, TEST_JWT_SECRET,
    };

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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
            assert_eq!(
                res.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "body {body:?} should be 422"
            );
        }
    }

    // --- archived filter helpers ---

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

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json.as_array().unwrap().len(),
            0,
            "archived monitor must not appear in default list"
        );
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json.as_array().unwrap().len(),
            1,
            "archived monitor must appear with include_archived=true"
        );
        assert_eq!(json[0]["status"], "archived");
    }

    // --- get_monitor tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn get_monitor_success(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], mid.to_string());
        assert_eq!(json["status"], "active");
        assert_eq!(json["kind"], "uptime");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_monitor_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors/{}", Uuid::new_v4()),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_monitor_wrong_workspace(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid1 = create_test_workspace(&pool, uid).await;
        let wid2 = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid1).await;

        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid2}/monitors/{mid}"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn get_monitor_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;
        let mid = create_test_monitor(&pool, wid).await;
        let res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid2,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_monitor_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(&format!(
                        "/api/workspaces/{}/monitors/{}",
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

    // --- patch_monitor tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_name(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({"name": "Renamed"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "Renamed");
        assert_eq!(json["url"], "https://example.com");
        assert_eq!(json["status"], "active");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_pause_and_resume(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool.clone(),
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({"status": "paused"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["status"],
            "paused"
        );

        let res2 = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({"status": "active"})),
        )
        .await;
        assert_eq!(res2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(res2.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body2).unwrap()["status"],
            "active"
        );
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_thresholds(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({"failure_threshold": 3, "recovery_threshold": 2})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["failure_threshold"], 3);
        assert_eq!(json["recovery_threshold"], 2);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_archived_status_rejected(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({"status": "archived"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_on_archived_rejected(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;
        archive_monitor(&pool, mid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({"name": "X"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_empty_body_rejected(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_invalid_interval(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            Some(json!({"interval_secs": 0})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{}", Uuid::new_v4()),
            uid,
            Some(json!({"name": "X"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn patch_monitor_not_member(pool: PgPool) {
        let uid1 = create_test_user(&pool).await;
        let uid2 = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid1).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool,
            Method::PATCH,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid2,
            Some(json!({"name": "X"})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn patch_monitor_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(&format!(
                        "/api/workspaces/{}/monitors/{}",
                        Uuid::new_v4(),
                        Uuid::new_v4()
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"X"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // --- delete_monitor tests ---

    async fn create_test_member(pool: &PgPool, workspace_id: Uuid) -> Uuid {
        let user_id = create_test_user(pool).await;
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(workspace_id)
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
        user_id
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_monitor_owner_archives(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res = authed(
            pool.clone(),
            Method::DELETE,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        let list_res = authed(
            pool.clone(),
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors"),
            uid,
            None,
        )
        .await;
        let body = axum::body::to_bytes(list_res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );

        let archived_res = authed(
            pool,
            Method::GET,
            &format!("/api/workspaces/{wid}/monitors?include_archived=true"),
            uid,
            None,
        )
        .await;
        let body2 = axum::body::to_bytes(archived_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2[0]["id"], mid.to_string());
        assert_eq!(json2[0]["status"], "archived");
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_monitor_member_forbidden(pool: PgPool) {
        let uid_owner = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid_owner).await;
        let mid = create_test_monitor(&pool, wid).await;
        let uid_member = create_test_member(&pool, wid).await;

        let res = authed(
            pool,
            Method::DELETE,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid_member,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_monitor_idempotent(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        let mid = create_test_monitor(&pool, wid).await;

        let res1 = authed(
            pool.clone(),
            Method::DELETE,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            None,
        )
        .await;
        assert_eq!(res1.status(), StatusCode::NO_CONTENT);

        let res2 = authed(
            pool,
            Method::DELETE,
            &format!("/api/workspaces/{wid}/monitors/{mid}"),
            uid,
            None,
        )
        .await;
        assert_eq!(res2.status(), StatusCode::NO_CONTENT);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_monitor_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;

        let res = authed(
            pool,
            Method::DELETE,
            &format!("/api/workspaces/{wid}/monitors/{}", Uuid::new_v4()),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn delete_monitor_workspace_not_found(pool: PgPool) {
        let uid = create_test_user(&pool).await;

        let res = authed(
            pool,
            Method::DELETE,
            &format!(
                "/api/workspaces/{}/monitors/{}",
                Uuid::new_v4(),
                Uuid::new_v4()
            ),
            uid,
            None,
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_monitor_unauthenticated() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let res = app(pool, TEST_JWT_SECRET.to_string())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(&format!(
                        "/api/workspaces/{}/monitors/{}",
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
