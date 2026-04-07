use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};

use crate::models::{session, user};
use crate::server::AppState;

pub const COOKIE_NAME: &str = "sendword_session";

/// Authenticated user extracted from the session cookie.
/// Used as an Axum extractor on protected route handlers.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: String,
    pub username: String,
}

/// Rejection type for AuthUser extraction failures.
/// Always results in a 303 redirect to /login.
pub struct AuthRejection;

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        Redirect::to("/login").into_response()
    }
}

/// Extract a cookie value by name from request headers.
fn extract_cookie(parts: &Parts, name: &str) -> Option<String> {
    let header = parts.headers.get(axum::http::header::COOKIE)?;
    let header_str = header.to_str().ok()?;
    for pair in header_str.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(name) {
            let value = value.strip_prefix('=')?;
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // 1. Extract session token from cookie
        let token = extract_cookie(parts, COOKIE_NAME).ok_or_else(|| {
            tracing::debug!("auth: no session cookie");
            AuthRejection
        })?;

        // 2. Look up session in database
        let pool = state.db.pool();
        let session = session::find_by_token(pool, &token)
            .await
            .map_err(|e| {
                tracing::debug!(error = %e, "auth: session lookup failed");
                AuthRejection
            })?
            .ok_or_else(|| {
                tracing::debug!("auth: session not found or expired");
                AuthRejection
            })?;

        // 3. Look up user
        let found_user = user::get_by_id(pool, &session.user_id)
            .await
            .map_err(|e| {
                tracing::debug!(error = %e, "auth: user lookup failed");
                AuthRejection
            })?;

        Ok(AuthUser {
            user_id: found_user.id,
            username: found_user.username,
        })
    }
}

/// Cookie configuration for the session cookie.
/// Used by the login handler to set the cookie with the correct flags.
pub struct CookieConfig {
    pub secure: bool,
}

impl CookieConfig {
    /// Build cookie config from the current application config.
    pub fn from_app_state(state: &AppState) -> Self {
        let config = state.config.load();
        Self {
            secure: config.auth.secure_cookie,
        }
    }

    /// Format a Set-Cookie header value for the session token.
    pub fn session_cookie_header(&self, token: &str) -> String {
        let mut header = format!("{COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/");
        if self.secure {
            header.push_str("; Secure");
        }
        header
    }

    /// Format a Set-Cookie header value that clears the session cookie.
    pub fn clear_cookie_header(&self) -> String {
        let mut header =
            format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
        if self.secure {
            header.push_str("; Secure");
        }
        header
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, Method};

    fn make_parts_with_cookie(cookie: &str) -> Parts {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(cookie).unwrap(),
        );
        let (mut parts, _) = axum::http::Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(())
            .unwrap()
            .into_parts();
        parts.headers = headers;
        parts
    }

    #[test]
    fn extract_cookie_finds_single_cookie() {
        let parts = make_parts_with_cookie("sendword_session=abc123");
        assert_eq!(
            extract_cookie(&parts, "sendword_session"),
            Some("abc123".into())
        );
    }

    #[test]
    fn extract_cookie_finds_cookie_among_multiple() {
        let parts =
            make_parts_with_cookie("other=xyz; sendword_session=token123; another=456");
        assert_eq!(
            extract_cookie(&parts, "sendword_session"),
            Some("token123".into())
        );
    }

    #[test]
    fn extract_cookie_returns_none_when_missing() {
        let parts = make_parts_with_cookie("other=xyz");
        assert_eq!(extract_cookie(&parts, "sendword_session"), None);
    }

    #[test]
    fn extract_cookie_returns_none_for_empty_value() {
        let parts = make_parts_with_cookie("sendword_session=");
        assert_eq!(extract_cookie(&parts, "sendword_session"), None);
    }

    #[test]
    fn extract_cookie_returns_none_when_no_cookie_header() {
        let (parts, _) = axum::http::Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(())
            .unwrap()
            .into_parts();
        assert_eq!(extract_cookie(&parts, "sendword_session"), None);
    }

    #[test]
    fn extract_cookie_does_not_match_prefix_substring() {
        let parts = make_parts_with_cookie("other_sendword_session=bad");
        assert_eq!(extract_cookie(&parts, "sendword_session"), None);
    }

    #[test]
    fn extract_cookie_does_not_match_name_with_suffix() {
        let parts = make_parts_with_cookie("sendword_session_extra=bad");
        assert_eq!(extract_cookie(&parts, "sendword_session"), None);
    }

    #[test]
    fn session_cookie_header_without_secure() {
        let cfg = CookieConfig { secure: false };
        let header = cfg.session_cookie_header("tok123");
        assert!(header.contains("sendword_session=tok123"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));
        assert!(header.contains("Path=/"));
        assert!(!header.contains("Secure"));
    }

    #[test]
    fn session_cookie_header_with_secure() {
        let cfg = CookieConfig { secure: true };
        let header = cfg.session_cookie_header("tok123");
        assert!(header.contains("Secure"));
    }

    #[test]
    fn clear_cookie_header_sets_max_age_zero() {
        let cfg = CookieConfig { secure: false };
        let header = cfg.clear_cookie_header();
        assert!(header.contains("Max-Age=0"));
        assert!(header.contains("sendword_session="));
    }
}
