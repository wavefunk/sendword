use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use allowthem_core::{AllowThem, AuthClient};

use crate::config::AppConfig;
use crate::config_writer::ConfigWriter;
use crate::db::Db;

pub struct AppState {
    pub config: ArcSwap<AppConfig>,
    pub config_writer: ConfigWriter,
    pub db: Db,
    pub http_client: reqwest::Client,
    pub ath: AllowThem,
    pub auth_client: Arc<dyn AuthClient>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        config_path: impl Into<std::path::PathBuf>,
        db: Db,
        ath: AllowThem,
        auth_client: Arc<dyn AuthClient>,
    ) -> Arc<Self> {
        let config_path = config_path.into();
        let http_client = reqwest::Client::builder().build().unwrap_or_default();
        Arc::new(Self {
            config: ArcSwap::from_pointee(config),
            config_writer: ConfigWriter::new(config_path),
            db,
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

const SENDWORD_SCRIPT: &[u8] = include_bytes!("../static/js/sendword.js");

pub fn sendword_script_response() -> Response {
    let mut response = Body::from(SENDWORD_SCRIPT).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript"),
    );
    response
}

async fn sendword_script() -> Response {
    sendword_script_response()
}

pub fn sendword_static_assets_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route("/static/js/sendword.js", get(sendword_script))
}

pub fn static_assets_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .nest("/static/wavefunk", wavefunk_ui::axum::asset_router())
        .merge(sendword_static_assets_router())
}

pub fn router(state: Arc<AppState>, auth_router: Router) -> Router {
    Router::new()
        .merge(crate::routes::router())
        .merge(sendword_static_assets_router())
        .fallback(static_asset_or_404)
        .with_state(state)
        .merge(auth_router)
        .layer(TraceLayer::new_for_http())
}

async fn static_asset_or_404(uri: axum::http::Uri, request_headers: HeaderMap) -> Response {
    if uri.path().starts_with("/static/wavefunk/") {
        return wavefunk_asset_response(uri.path(), request_headers);
    }

    fallback_404().await.into_response()
}

fn wavefunk_asset_response(path: &str, request_headers: HeaderMap) -> Response {
    match wavefunk_ui::assets::get(path) {
        Some(asset) => {
            let etag = wavefunk_ui::assets::etag(&asset.path)
                .expect("embedded wavefunk-ui asset should have an entity tag");
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(asset.content_type),
            );
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(wavefunk_ui::assets::CACHE_CONTROL),
            );
            headers.insert(
                header::ETAG,
                HeaderValue::from_str(&etag)
                    .expect("wavefunk-ui asset entity tags should be valid headers"),
            );

            if if_none_match_matches(request_headers.get(header::IF_NONE_MATCH), &etag) {
                return (StatusCode::NOT_MODIFIED, headers).into_response();
            }

            (StatusCode::OK, headers, body_from_asset_bytes(asset.bytes)).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn body_from_asset_bytes(bytes: Cow<'static, [u8]>) -> Body {
    match bytes {
        Cow::Borrowed(bytes) => Body::from(bytes),
        Cow::Owned(bytes) => Body::from(bytes),
    }
}

fn if_none_match_matches(header: Option<&HeaderValue>, etag: &str) -> bool {
    header
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        })
}

async fn fallback_404() -> impl IntoResponse {
    let html = match crate::views::fallback::render_not_found_page() {
        Ok(Html(html)) => html,
        Err(err) => {
            tracing::error!(error = ?err, "failed to render 404 page");
            "404 - page not found".to_owned()
        }
    };

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
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::{StatusCode, header};
    use tower::ServiceExt;

    #[test]
    fn sendword_script_response_serves_asset() {
        let response = super::sendword_script_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/javascript")
        );
    }

    #[test]
    fn sendword_script_confirm_handler_runs_before_htmx_boosted_submit() {
        let script =
            std::str::from_utf8(super::SENDWORD_SCRIPT).expect("sendword.js is valid UTF-8");

        assert!(script.contains("document.addEventListener('submit', handleFormConfirm, true);"));
        assert!(script.contains("event.stopImmediatePropagation();"));
    }

    #[tokio::test]
    async fn static_assets_router_serves_wavefunk_ui_and_sendword_assets() {
        let app = super::static_assets_router::<()>();

        for (path, content_type) in [
            (
                "/static/wavefunk/css/wavefunk.css",
                "text/css; charset=utf-8",
            ),
            (
                "/static/wavefunk/js/wavefunk.js",
                "text/javascript; charset=utf-8",
            ),
            (
                "/static/wavefunk/js/htmx.min.js",
                "text/javascript; charset=utf-8",
            ),
            (
                "/static/wavefunk/js/htmx-sse.js",
                "text/javascript; charset=utf-8",
            ),
            (
                "/static/wavefunk/css/fonts/MartianGrotesk-VF.woff2",
                "font/woff2",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some(content_type),
                "{path}"
            );
        }

        let sendword_response = app
            .oneshot(
                Request::builder()
                    .uri("/static/js/sendword.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sendword_response.status(), StatusCode::OK);
        assert_eq!(
            sendword_response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/javascript")
        );
    }

    #[tokio::test]
    async fn static_assets_router_404s_missing_sendword_asset() {
        let app = super::static_assets_router::<()>();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/static/js/missing.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
