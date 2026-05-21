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

use crate::{authz, middleware::CurrentUser, AppState};

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
    if let Err(e) = authz::check_membership(&state.pool, workspace_id, current_user.id).await {
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
    if let Err(e) = authz::check_membership(&state.pool, workspace_id, current_user.id).await {
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
        authed, create_test_monitor, create_test_monitor_thresholds, create_test_user,
        create_test_workspace, TEST_JWT_SECRET,
    };

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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
            &format!(
                "/api/workspaces/{wid}/monitors/{}/check-results",
                Uuid::new_v4()
            ),
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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

        sqlx::query("INSERT INTO check_results (monitor_id, status) VALUES ($1, 'up')")
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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

        // First down — pre-inserted directly, threshold not yet crossed
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

        // Pre-insert a degraded result — breaks any pure-down streak
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
        // failure_threshold = 1: first down opens an incident
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

        // Second down — incident already open, should NOT open a second one
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

    async fn closed_incident_count(pool: &PgPool, monitor_id: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM incidents WHERE monitor_id = $1 AND closed_at IS NOT NULL",
        )
        .bind(monitor_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

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
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 1).await;

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
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 1).await;

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

    // --- incident close tests ---

    #[sqlx::test(migrations = "../migrations")]
    async fn close_incident_after_recovery(pool: PgPool) {
        let uid = create_test_user(&pool).await;
        let wid = create_test_workspace(&pool, uid).await;
        // failure_threshold = 1, recovery_threshold = 2
        let mid = create_test_monitor_thresholds(&pool, wid, 1, 2).await;

        // Open an incident by inserting a down result at a known old time and opening the incident directly
        sqlx::query(
            "INSERT INTO check_results (monitor_id, status, checked_at) \
             VALUES ($1, 'down', NOW() - INTERVAL '20 seconds')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO incidents (monitor_id) VALUES ($1)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
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
}
