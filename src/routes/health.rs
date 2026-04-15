use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::get;
use serde_json::{Value, json};

use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/healthz", get(healthz))
}

async fn healthz(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Value>) {
    match sqlx::query("SELECT 1").execute(state.db.pool()).await {
        Ok(_) => (StatusCode::OK, Json(json!({"status": "ok"}))),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "error", "detail": e.to_string()})),
        ),
    }
}
