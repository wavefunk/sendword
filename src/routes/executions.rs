use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;

use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/executions/{id}", get(execution_detail))
        .route("/executions/{id}/replay", post(replay_execution))
}

async fn execution_detail(Path(_id): Path<String>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

async fn replay_execution(Path(_id): Path<String>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
