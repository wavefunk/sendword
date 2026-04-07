use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use axum::Router;

use crate::error::AppError;
use crate::models::execution;
use crate::server::AppState;
use crate::templates::context;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(dashboard))
}

async fn dashboard(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let config = state.config.load();
    let pool = state.db.pool();

    let mut hooks = Vec::with_capacity(config.hooks.len());
    for h in &config.hooks {
        let last = match execution::get_latest_by_hook(pool, &h.slug).await {
            Ok(exec) => exec,
            Err(e) => {
                tracing::warn!(hook = %h.slug, error = %e, "failed to fetch last execution");
                None
            }
        };

        hooks.push(context! {
            name => h.name,
            slug => h.slug,
            description => h.description,
            enabled => h.enabled,
            last_status => last.as_ref().map(|e| e.status.to_string()),
            last_triggered_at => last.as_ref().map(|e| &e.triggered_at),
            last_execution_id => last.as_ref().map(|e| &e.id),
        });
    }

    let html = state
        .templates
        .render("dashboard.html", context! { hooks => hooks })?;
    Ok(Html(html))
}
