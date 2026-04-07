use std::sync::Arc;

use axum::Router;

use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
}
