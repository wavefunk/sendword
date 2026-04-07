mod auth;
mod dashboard;
mod executions;
mod health;
mod hooks;

use std::sync::Arc;

use axum::Router;

use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(health::router())
        .merge(auth::router())
        .merge(dashboard::router())
        .merge(hooks::router())
        .merge(executions::router())
}
