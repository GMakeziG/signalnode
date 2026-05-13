pub mod password;
pub mod token;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use uuid::Uuid;

use crate::AppState;
use password::{hash_password, verify_password};
use token::{encode_access_token, encode_refresh_token};

static DUMMY_HASH: OnceLock<String> = OnceLock::new();

fn dummy_hash() -> &'static str {
    DUMMY_HASH.get_or_init(|| {
        hash_password("dummy_password_for_timing_mitigation")
            .expect("dummy hash init")
    })
}

fn is_valid_email(email: &str) -> bool {
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    !local.is_empty() && domain.contains('.')
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> impl IntoResponse {
    if !is_valid_email(&body.email) || body.password.len() < 8 {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let password = body.password.clone();
    let hash = match tokio::task::spawn_blocking(move || hash_password(&password)).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::error!(error = ?e, "password hashing failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "spawn_blocking panicked during hash");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let result = sqlx::query(
        "INSERT INTO users (id, email, password_hash) VALUES ($1, $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(&body.email)
    .bind(&hash)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            StatusCode::CONFLICT.into_response()
        }
        Err(e) => {
            tracing::error!(error = ?e, "database error during register");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let row = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, password_hash FROM users WHERE email = $1",
    )
    .bind(&body.email)
    .fetch_optional(&state.pool)
    .await;

    let (user_id, password_hash) = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            // Dummy verify to equalise timing with the found-user path
            let _ = tokio::task::spawn_blocking(|| verify_password("x", dummy_hash())).await;
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "database error during login");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let password = body.password.clone();
    let ok = match tokio::task::spawn_blocking(move || verify_password(&password, &password_hash)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::error!(error = ?e, "password verification error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(e) => {
            tracing::error!(error = ?e, "spawn_blocking panicked during verify");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let uid = user_id.to_string();
    let access_token = match encode_access_token(&uid, &state.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = ?e, "access token encoding failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let refresh_token = match encode_refresh_token(&uid, &state.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = ?e, "refresh token encoding failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    Json(AuthResponse { access_token, refresh_token }).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use serde_json::json;
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::app;

    const TEST_JWT_SECRET: &str = "test-secret-at-least-32-chars-long!";

    async fn post_json(pool: PgPool, uri: &str, body: serde_json::Value) -> axum::response::Response {
        let app = app(pool, TEST_JWT_SECRET.to_string());
        app.oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn register_success(pool: PgPool) {
        let res = post_json(
            pool,
            "/auth/register",
            json!({"email": "user@example.com", "password": "securepass123"}),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn register_duplicate_email(pool: PgPool) {
        let pool2 = pool.clone();
        post_json(pool, "/auth/register", json!({"email": "dup@example.com", "password": "password123"})).await;
        let res = post_json(pool2, "/auth/register", json!({"email": "dup@example.com", "password": "password456"})).await;
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_success_returns_tokens(pool: PgPool) {
        let pool2 = pool.clone();
        post_json(pool, "/auth/register", json!({"email": "login@example.com", "password": "mypassword1"})).await;
        let res = post_json(pool2, "/auth/login", json!({"email": "login@example.com", "password": "mypassword1"})).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["access_token"].is_string());
        assert!(json["refresh_token"].is_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_wrong_password_rejected(pool: PgPool) {
        let pool2 = pool.clone();
        post_json(pool, "/auth/register", json!({"email": "bad@example.com", "password": "correct123"})).await;
        let res = post_json(pool2, "/auth/login", json!({"email": "bad@example.com", "password": "wrongpass1"})).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_unknown_user_rejected(pool: PgPool) {
        let res = post_json(pool, "/auth/login", json!({"email": "ghost@example.com", "password": "somepass1"})).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn register_rejects_empty_email(pool: PgPool) {
        let res = post_json(pool, "/auth/register", json!({"email": "", "password": "securepass123"})).await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn register_rejects_invalid_email(pool: PgPool) {
        let res = post_json(pool, "/auth/register", json!({"email": "notanemail", "password": "securepass123"})).await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn register_rejects_short_password(pool: PgPool) {
        let res = post_json(pool, "/auth/register", json!({"email": "user@example.com", "password": "short"})).await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
