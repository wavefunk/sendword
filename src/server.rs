use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::config::AppConfig;
use crate::db::Db;
use crate::templates::Templates;

pub struct AppState {
    pub config: AppConfig,
    pub db: Db,
    pub templates: Templates,
}

impl AppState {
    pub fn new(config: AppConfig, db: Db, templates: Templates) -> Arc<Self> {
        Arc::new(Self {
            config,
            db,
            templates,
        })
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let static_dir = ServeDir::new("static");

    Router::new()
        .merge(crate::routes::router())
        .nest_service("/static", static_dir)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn run(state: Arc<AppState>) -> eyre::Result<()> {
    let addr = format!("{}:{}", state.config.server.bind, state.config.server.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "server listening");

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("server shut down");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT"),
        _ = terminate => tracing::info!("received SIGTERM"),
    }
}
