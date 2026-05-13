pub mod auth;

use axum::{http::StatusCode, routing::get, Router};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_secret: String,
}

pub fn app(pool: PgPool, jwt_secret: String) -> Router {
    assert!(!jwt_secret.is_empty(), "JWT_SECRET must be set and non-empty");
    let state = AppState { pool, jwt_secret };
    Router::new()
        .route("/health", get(health))
        .nest("/auth", auth::router())
        .with_state(state)
}

async fn health() -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_200() {
        let pool = PgPool::connect_lazy("postgres://unused").unwrap();
        let response = app(pool, "test-secret-at-least-32-chars-long!".to_string())
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
