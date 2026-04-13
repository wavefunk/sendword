use std::sync::Arc;

use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Form;
use axum::Router;
use serde::Deserialize;

use crate::auth::{CookieConfig, COOKIE_NAME};
use crate::models::{session, user};
use crate::server::AppState;
use crate::templates::context;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", get(login_page).post(login_submit))
        .route("/logout", get(logout))
}

/// GET /login — render login form.
/// If the user already has a valid session, redirect to /.
async fn login_page(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    // If the user already has a valid session, redirect to dashboard
    if has_valid_session(&state, &headers).await {
        return Ok(Redirect::to("/").into_response());
    }

    render_login(&state, None)
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

/// POST /login — validate credentials, create session, set cookie, redirect to /.
async fn login_submit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<LoginForm>,
) -> Result<Response, Response> {
    // Look up user by username
    let pool = state.db.pool();
    let found_user = match user::get_by_username(pool, &form.username).await {
        Ok(u) => u,
        Err(_) => return render_login(&state, Some("Invalid username or password")),
    };

    // Verify password
    if !user::verify_password(&form.password, &found_user.password_hash) {
        return render_login(&state, Some("Invalid username or password"));
    }

    // Create session
    let session_lifetime = state.config.load().auth.session_lifetime;
    let sess = session::create(pool, &found_user.id, session_lifetime)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to create session");
            render_login_unwrap(&state, Some("Login failed, please try again"))
        })?;

    // Set cookie and redirect
    let cookie_config = CookieConfig::from_app_state(&state);
    let cookie_header = cookie_config.session_cookie_header(&sess.id);

    Ok(([(SET_COOKIE, cookie_header)], Redirect::to("/")).into_response())
}

/// GET /logout — delete session, clear cookie, redirect to /login.
async fn logout(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    // Extract session token from cookie header
    if let Some(cookie_header) = headers.get(axum::http::header::COOKIE)
        && let Ok(cookie_str) = cookie_header.to_str()
            && let Some(token) = extract_session_token(cookie_str) {
                let pool = state.db.pool();
                if let Err(e) = session::delete(pool, token).await {
                    tracing::warn!(error = %e, "failed to delete session during logout");
                }
            }

    // Clear cookie and redirect
    let cookie_config = CookieConfig::from_app_state(&state);
    let clear_header = cookie_config.clear_cookie_header();

    ([(SET_COOKIE, clear_header)], Redirect::to("/login")).into_response()
}

/// Extract the session token value from a Cookie header string.
fn extract_session_token(cookie_str: &str) -> Option<&str> {
    for pair in cookie_str.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(COOKIE_NAME) {
            let value = value.strip_prefix('=')?;
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Check whether the request carries a valid, non-expired session cookie.
async fn has_valid_session(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let cookie_header = match headers.get(axum::http::header::COOKIE) {
        Some(h) => h,
        None => return false,
    };
    let cookie_str = match cookie_header.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let token = match extract_session_token(cookie_str) {
        Some(t) => t,
        None => return false,
    };

    matches!(
        session::find_by_token(state.db.pool(), token).await,
        Ok(Some(_))
    )
}

/// Render the login template with an optional error message.
#[allow(clippy::result_large_err)]
fn render_login(state: &AppState, error: Option<&str>) -> Result<Response, Response> {
    let html = state
        .templates
        .render("login.html", context! { error => error })
        .map_err(|e| {
            tracing::error!(error = %e, "failed to render login template");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;
    Ok(Html(html).into_response())
}

/// Render login template, panicking on template failure.
/// Only used in error-mapping contexts where we already need a Response.
fn render_login_unwrap(state: &AppState, error: Option<&str>) -> Response {
    match render_login(state, error) {
        Ok(resp) => resp,
        Err(resp) => resp,
    }
}
