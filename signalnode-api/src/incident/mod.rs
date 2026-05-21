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

use crate::{authz, middleware::CurrentUser, AppState};

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

async fn list_open_incidents(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = authz::check_membership(&state.pool, workspace_id, current_user.id).await {
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

        // One closed incident
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
}
