pub mod auth;

use axum::{http::StatusCode, routing::get, Router};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_secret: String,
}

pub fn app(pool: PgPool) -> Router {
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_default();
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
        let response = app(pool)
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
