use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State, connect_info::IntoMakeServiceWithConnectInfo};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use minijinja::context;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use allowthem_core::{AllowThem, AuthClient};

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
    pub ath: AllowThem,
    pub auth_client: Arc<dyn AuthClient>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        config_path: impl Into<std::path::PathBuf>,
        db: Db,
        templates: Templates,
        ath: AllowThem,
        auth_client: Arc<dyn AuthClient>,
    ) -> Arc<Self> {
        let config_path = config_path.into();
        let http_client = reqwest::Client::builder().build().unwrap_or_default();
        Arc::new(Self {
            config: ArcSwap::from_pointee(config),
            config_writer: ConfigWriter::new(config_path),
            db,
            templates,
            http_client,
            ath,
            auth_client,
        })
    }

    /// Reload the config from the TOML file path associated with this state.
    ///
    /// Reads and validates the config file, then atomically swaps the live
    /// config. Returns an error if the file cannot be read or fails validation.
    pub fn reload_config(&self) -> Result<(), crate::config::ConfigError> {
        let path_str = self
            .config_writer
            .path()
            .to_str()
            .unwrap_or("sendword.toml");
        let new_config = AppConfig::load_from(path_str, "nonexistent.json")?;
        self.config.store(Arc::new(new_config));
        Ok(())
    }
}

#[derive(rust_embed::RustEmbed)]
#[folder = "static"]
struct StaticAssets;

pub fn embedded_static_response(path: &str) -> Response {
    let path = path.trim_start_matches('/');
    let Some(file) = StaticAssets::get(path) else {
        return (StatusCode::NOT_FOUND, "static asset not found").into_response();
    };

    let content_type = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Body::from(file.data.into_owned()).into_response();
    let header_value = HeaderValue::from_str(content_type.as_ref())
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, header_value);
    response
}

async fn static_asset(Path(path): Path<String>) -> Response {
    embedded_static_response(&path)
}

pub fn router(state: Arc<AppState>, auth_router: Router) -> Router {
    Router::new()
        .merge(crate::routes::router())
        .route("/static/{*path}", get(static_asset))
        .fallback(fallback_404)
        .with_state(state)
        .merge(auth_router)
        .layer(TraceLayer::new_for_http())
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
    auth_router: Router,
) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    router(state, auth_router).into_make_service_with_connect_info::<SocketAddr>()
}

pub async fn run(state: Arc<AppState>, auth_router: Router) -> eyre::Result<()> {
    let config = state.config.load();
    let addr = format!("{}:{}", config.server.bind, config.server.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "server listening");

    axum::serve(listener, into_service(state, auth_router))
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

#[cfg(test)]
mod tests {
    use super::embedded_static_response;
    use axum::http::{StatusCode, header};

    #[test]
    fn embedded_static_response_serves_css_asset() {
        let response = embedded_static_response("css/wavefunk.css");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/css")
        );
    }

    #[test]
    fn embedded_static_response_404s_missing_asset() {
        let response = embedded_static_response("missing.css");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
