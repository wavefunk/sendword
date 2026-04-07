use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use axum::Router;

use crate::error::AppError;
use crate::server::AppState;
use crate::templates::context;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(dashboard))
}

async fn dashboard(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let hooks: Vec<_> = state
        .config
        .hooks
        .iter()
        .map(|h| {
            context! {
                name => h.name,
                slug => h.slug,
                description => h.description,
                enabled => h.enabled,
            }
        })
        .collect();

    let html = state
        .templates
        .render("dashboard.html", context! { hooks => hooks })?;
    Ok(Html(html))
}
