use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::borrow::Cow;

use crate::ErrorBody;

pub enum WorkspaceError {
    SlugTaken,
    InvalidInput(String),
    Internal,
}

impl IntoResponse for WorkspaceError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            WorkspaceError::SlugTaken => (
                StatusCode::CONFLICT,
                "slug_taken",
                Cow::Borrowed("A workspace with that slug already exists"),
            ),
            WorkspaceError::InvalidInput(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Cow::Owned(msg))
            }
            WorkspaceError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                Cow::Borrowed("An internal error occurred"),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}
