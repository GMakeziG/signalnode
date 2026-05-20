use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum CheckResultError {
    Forbidden,
    NotFound,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for CheckResultError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            CheckResultError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            CheckResultError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            CheckResultError::InvalidInput(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Cow::Owned(msg))
            }
            CheckResultError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
