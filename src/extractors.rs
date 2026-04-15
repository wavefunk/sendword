use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};

use allowthem_core::{User, parse_session_cookie};

use crate::server::AppState;

/// Sendword-local AuthUser extractor.
///
/// Validates the allowthem session cookie and returns the authenticated user.
/// On failure, redirects to `/login` (303) instead of returning 401 JSON —
/// appropriate for HTML-serving routes.
pub struct AuthUser(pub User);

pub struct AuthRejection;

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        Redirect::to("/login").into_response()
    }
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let cookie_header = parts
            .headers
            .get(COOKIE)
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthRejection)?;

        let cookie_name = state.auth_client.session_cookie_name();
        let token = parse_session_cookie(cookie_header, cookie_name).ok_or(AuthRejection)?;

        let user = state
            .auth_client
            .validate_session(&token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "session validation error");
                AuthRejection
            })?
            .ok_or(AuthRejection)?;

        Ok(AuthUser(user))
    }
}
