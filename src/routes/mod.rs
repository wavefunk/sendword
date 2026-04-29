mod api;
mod dashboard;
mod executions;
mod health;
mod hooks;
mod scripts;
mod settings;

use std::sync::Arc;

use axum::Router;

use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(api::router())
        .merge(health::router())
        .merge(dashboard::router())
        .merge(hooks::router())
        .merge(executions::router())
        .merge(scripts::router())
        .merge(settings::router())
}
