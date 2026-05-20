use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub enum AuthError {
    InvalidCredentials,
    InvalidToken,
    EmailTaken,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AuthError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "Invalid email or password".to_string(),
            ),
            AuthError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "Token is invalid or expired".to_string(),
            ),
            AuthError::EmailTaken => (
                StatusCode::CONFLICT,
                "email_taken",
                "An account with that email already exists".to_string(),
            ),
            AuthError::InvalidInput(msg) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", msg),
            AuthError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An internal error occurred".to_string(),
            ),
        };
        (status, Json(json!({"code": code, "message": message}))).into_response()
    }
}
