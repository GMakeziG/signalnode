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
use uuid::Uuid;

use crate::AppState;
use password::{hash_password, verify_password};
use token::{encode_access_token, encode_refresh_token};

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
    let hash = match hash_password(&body.password) {
        Ok(h) => h,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let ok = match verify_password(&body.password, &password_hash) {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if !ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let uid = user_id.to_string();
    let access_token = match encode_access_token(&uid, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let refresh_token = match encode_refresh_token(&uid, &state.jwt_secret) {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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

    async fn post_json(pool: PgPool, uri: &str, body: serde_json::Value) -> axum::response::Response {
        let app = app(pool);
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
        post_json(pool, "/auth/register", json!({"email": "dup@example.com", "password": "pass1"})).await;
        let res = post_json(pool2, "/auth/register", json!({"email": "dup@example.com", "password": "pass2"})).await;
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_success_returns_tokens(pool: PgPool) {
        let pool2 = pool.clone();
        post_json(pool, "/auth/register", json!({"email": "login@example.com", "password": "mypassword"})).await;
        let res = post_json(pool2, "/auth/login", json!({"email": "login@example.com", "password": "mypassword"})).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["access_token"].is_string());
        assert!(json["refresh_token"].is_string());
    }

    #[sqlx::test(migrations = "../migrations")]
    async fn login_wrong_password_rejected(pool: PgPool) {
        let pool2 = pool.clone();
        post_json(pool, "/auth/register", json!({"email": "bad@example.com", "password": "correct"})).await;
        let res = post_json(pool2, "/auth/login", json!({"email": "bad@example.com", "password": "wrong"})).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
