use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;

use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hook/{slug}", post(trigger_hook))
        .route("/hooks/{slug}", get(hook_detail))
}

async fn trigger_hook(Path(_slug): Path<String>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

async fn hook_detail(Path(_slug): Path<String>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
