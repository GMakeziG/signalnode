use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum MonitorError {
    Forbidden,
    NotFound,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for MonitorError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            MonitorError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                Cow::Borrowed("You do not have access to this resource"),
            ),
            MonitorError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                Cow::Borrowed("The requested resource was not found"),
            ),
            MonitorError::InvalidInput(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Cow::Owned(msg))
            }
            MonitorError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
