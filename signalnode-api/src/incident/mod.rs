use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{middleware::CurrentUser, AppState};

#[derive(Serialize, sqlx::FromRow)]
struct Incident {
    id: Uuid,
    monitor_id: Uuid,
    opened_at: DateTime<Utc>,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/workspaces/{workspace_id}/incidents",
        get(list_open_incidents),
    )
}

async fn list_open_incidents(
    _state: State<AppState>,
    _current_user: CurrentUser,
    _path: Path<Uuid>,
) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}
