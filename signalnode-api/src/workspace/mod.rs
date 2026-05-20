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

mod error;
use error::WorkspaceError;

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
    slug.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

async fn create_workspace(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<CreateWorkspaceRequest>,
) -> impl IntoResponse {
    if body.name.is_empty() {
        return WorkspaceError::InvalidInput("Name must not be empty".into()).into_response();
    }
    if !is_valid_slug(&body.slug) {
        return WorkspaceError::InvalidInput(
            "Slug must be lowercase alphanumeric, optionally separated by hyphens".into(),
        )
        .into_response();
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = ?e, "failed to begin transaction");
            return WorkspaceError::Internal.into_response();
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
        Err(sqlx::Error::Database(e))
            if e.is_unique_violation()
                && e.constraint() == Some("workspaces_slug_key") =>
        {
            return WorkspaceError::SlugTaken.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to insert workspace");
            return WorkspaceError::Internal.into_response();
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
        return WorkspaceError::Internal.into_response();
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
            return WorkspaceError::Internal.into_response();
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!(error = ?e, "failed to commit transaction");
        return WorkspaceError::Internal.into_response();
    }

    (StatusCode::CREATED, Json(workspace)).into_response()
}

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
            WorkspaceError::Internal.into_response()
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
    use crate::test_helpers::{authed, create_test_user, TEST_JWT_SECRET};

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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let row: (Uuid, String) =
            sqlx::query_as("SELECT user_id, role FROM workspace_members WHERE user_id = $1")
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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

    #[sqlx::test(migrations = "../migrations")]
    async fn create_workspace_duplicate_slug_returns_structured_error(pool: PgPool) {
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "slug_taken");
    }
}
