use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::extract::{connect_info::IntoMakeServiceWithConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::Router;
use minijinja::context;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::config::AppConfig;
use crate::config_writer::ConfigWriter;
use crate::db::Db;
use crate::templates::Templates;

pub struct AppState {
    pub config: ArcSwap<AppConfig>,
    pub config_writer: ConfigWriter,
    pub db: Db,
    pub templates: Templates,
    pub http_client: reqwest::Client,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        config_path: impl Into<std::path::PathBuf>,
        db: Db,
        templates: Templates,
    ) -> Arc<Self> {
        let config_path = config_path.into();
        let http_client = reqwest::Client::builder()
            .build()
            .unwrap_or_default();
        Arc::new(Self {
            config: ArcSwap::from_pointee(config),
            config_writer: ConfigWriter::new(config_path),
            db,
            templates,
            http_client,
        })
    }

    /// Reload the config from the TOML file path associated with this state.
    ///
    /// Reads and validates the config file, then atomically swaps the live
    /// config. Returns an error if the file cannot be read or fails validation.
    pub fn reload_config(&self) -> Result<(), crate::config::ConfigError> {
        let path_str = self.config_writer.path().to_str().unwrap_or("sendword.toml");
        let new_config = AppConfig::load_from(path_str, "nonexistent.json")?;
        self.config.store(Arc::new(new_config));
        Ok(())
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let static_dir = ServeDir::new("static");

    Router::new()
        .merge(crate::routes::router())
        .nest_service("/static", static_dir)
        .fallback(fallback_404)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn fallback_404(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let html = state
        .templates
        .render(
            "404.html",
            context! {
                nav_active => "",
            },
        )
        .unwrap_or_else(|_| "404 — page not found".to_owned());
    (StatusCode::NOT_FOUND, Html(html))
}

/// Build the router as a service that provides `ConnectInfo<SocketAddr>` to
/// handlers. Use this when serving via `axum::serve`.
pub fn into_service(
    state: Arc<AppState>,
) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    router(state).into_make_service_with_connect_info::<SocketAddr>()
}

pub async fn run(state: Arc<AppState>) -> eyre::Result<()> {
    let config = state.config.load();
    let addr = format!("{}:{}", config.server.bind, config.server.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "server listening");

    axum::serve(listener, into_service(state))
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
