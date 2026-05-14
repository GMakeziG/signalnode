pub mod auth;
pub mod middleware;
pub mod workspace;

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use middleware::CurrentUser;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_secret: String,
}

pub fn app(pool: PgPool, jwt_secret: String) -> Router {
    assert!(!jwt_secret.is_empty(), "JWT_SECRET must be set and non-empty");
    let state = AppState { pool, jwt_secret };

    let protected = Router::new()
        .route("/api/me", get(me))
        .nest("/api", workspace::router())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ));

    Router::new()
        .route("/health", get(health))
        .nest("/auth", auth::router())
        .merge(protected)
        .with_state(state)
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn me(current_user: CurrentUser) -> impl IntoResponse {
    Json(serde_json::json!({ "id": current_user.id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::auth::token::{encode_access_token, encode_refresh_token};

    const TEST_JWT: &str = "test-secret-at-least-32-chars-long!";
    const TEST_UID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn test_app() -> Router {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        app(pool, TEST_JWT.to_string())
    }

    #[tokio::test]
    async fn health_returns_200() {
        let response = test_app()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn me_missing_token_returns_401() {
        let res = test_app()
            .oneshot(Request::builder().uri("/api/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_invalid_token_returns_401() {
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("Authorization", "Bearer notavalidtoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_refresh_token_rejected() {
        let token = encode_refresh_token(TEST_UID, TEST_JWT).unwrap();
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_valid_access_token_accepted() {
        let token = encode_access_token(TEST_UID, TEST_JWT).unwrap();
        let res = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], TEST_UID);
    }
}
