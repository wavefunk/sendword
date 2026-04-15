use std::sync::Arc;

use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Form;
use axum::Router;
use chrono::Utc;
use serde::Deserialize;

use allowthem_core::{generate_token, hash_token, password};

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
    // Check if already authenticated via OptionalAuthUser logic
    if let Some(cookie_header) = headers.get(axum::http::header::COOKIE)
        && let Ok(cookie_str) = cookie_header.to_str()
    {
        let cookie_name = state.auth_client.session_cookie_name();
        if let Some(token) = allowthem_core::parse_session_cookie(cookie_str, cookie_name) {
            if state
                .auth_client
                .validate_session(&token)
                .await
                .unwrap_or(None)
                .is_some()
            {
                return Ok(Redirect::to("/").into_response());
            }
        }
    }

    render_login(&state, None)
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

/// POST /login — validate credentials, create session, set cookie, redirect to /.
async fn login_submit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<LoginForm>,
) -> Result<Response, Response> {
    // Look up user with password hash populated
    let found_user = match state.ath.db().find_for_login(&form.email).await {
        Ok(u) => u,
        Err(_) => return render_login(&state, Some("Invalid email or password")),
    };

    // Verify password — find_for_login returns password_hash populated
    let Some(pw_hash) = &found_user.password_hash else {
        return render_login(&state, Some("Invalid email or password"));
    };

    match password::verify_password(&form.password, pw_hash) {
        Ok(true) => {}
        Ok(false) => return render_login(&state, Some("Invalid email or password")),
        Err(e) => {
            tracing::error!(error = %e, "password verification error");
            return render_login(&state, Some("Login failed, please try again"));
        }
    }

    // Generate token, hash for storage
    let token = generate_token();
    let token_hash = hash_token(&token);

    let ttl = state.ath.session_config().ttl;
    let expires = Utc::now() + ttl;

    if let Err(e) = state
        .ath
        .db()
        .create_session(found_user.id, token_hash, None, None, expires)
        .await
    {
        tracing::error!(error = %e, "failed to create session");
        return render_login(&state, Some("Login failed, please try again"));
    }

    let cookie_header = state.ath.session_cookie(&token);
    Ok(([(SET_COOKIE, cookie_header)], Redirect::to("/")).into_response())
}

/// GET /logout — delete session, clear cookie, redirect to /login.
async fn logout(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Some(cookie_header) = headers.get(axum::http::header::COOKIE)
        && let Ok(cookie_str) = cookie_header.to_str()
    {
        let cookie_name = state.auth_client.session_cookie_name();
        if let Some(token) = allowthem_core::parse_session_cookie(cookie_str, cookie_name) {
            if let Err(e) = state.auth_client.logout(&token).await {
                tracing::warn!(error = %e, "failed to delete session during logout");
            }
        }
    }

    // Build a clearing cookie: same name, Max-Age=0
    let config = state.ath.session_config();
    let clear_cookie = format!(
        "{}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{}",
        config.cookie_name,
        if config.secure { "; Secure" } else { "" },
    );

    ([(SET_COOKIE, clear_cookie)], Redirect::to("/login")).into_response()
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
