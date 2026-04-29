use std::sync::Arc;

use axum::Form;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use uuid::Uuid;

use allowthem_core::{AuthError, Email, UserId};

use crate::error::AppError;
use crate::extractors::AuthUser;
use crate::server::AppState;
use crate::templates::context;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/users", get(list_users).post(create_user))
        .route(
            "/admin/users/{id}/delete",
            axum::routing::post(delete_user),
        )
}

// --- Query params for flash messages ---

#[derive(Deserialize)]
struct FlashParams {
    success: Option<String>,
    error: Option<String>,
}

// --- GET /admin/users ---

async fn list_users(
    AuthUser(auth): AuthUser,
    State(state): State<Arc<AppState>>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let all_users = state.ath.db().list_users().await?;

    let user_rows: Vec<_> = all_users
        .iter()
        .map(|u| {
            context! {
                id => u.id.to_string(),
                // "username" key keeps existing template working until commit 8 updates login.html
                username => u.email.as_str(),
                created_at => u.created_at.to_rfc3339(),
                is_self => u.id == auth.id,
            }
        })
        .collect();

    let html = state.templates.render(
        "users.html",
        context! {
            users => user_rows,
            success => flash.success,
            error => flash.error,
            username => auth.email.as_str(),
            nav_active => "admin",
        },
    )?;
    Ok(Html(html))
}

// --- POST /admin/users ---

#[derive(Deserialize)]
struct CreateUserForm {
    email: String,
    password: String,
}

async fn create_user(
    AuthUser(_auth): AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreateUserForm>,
) -> Response {
    if form.password.is_empty() {
        return Redirect::to("/admin/users?error=Password+cannot+be+empty").into_response();
    }

    let email = match Email::new(form.email.clone()) {
        Ok(e) => e,
        Err(_) => {
            let encoded = urlencoding::encode("Invalid email address");
            return Redirect::to(&format!("/admin/users?error={encoded}")).into_response();
        }
    };

    match state
        .ath
        .db()
        .create_user(email, &form.password, None, None)
        .await
    {
        Ok(created) => {
            let msg = format!("User '{}' created", created.email.as_str());
            let encoded = urlencoding::encode(&msg);
            Redirect::to(&format!("/admin/users?success={encoded}")).into_response()
        }
        Err(AuthError::Conflict(msg)) => {
            let encoded = urlencoding::encode(&msg);
            Redirect::to(&format!("/admin/users?error={encoded}")).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to create user");
            Redirect::to("/admin/users?error=Failed+to+create+user").into_response()
        }
    }
}

// --- POST /admin/users/:id/delete ---

async fn delete_user(
    AuthUser(auth): AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let user_id = match Uuid::parse_str(&id).map(UserId::from_uuid) {
        Ok(uid) => uid,
        Err(_) => {
            return Redirect::to("/admin/users?error=Invalid+user+ID").into_response();
        }
    };

    // Prevent self-deletion
    if user_id == auth.id {
        return Redirect::to("/admin/users?error=Cannot+delete+yourself").into_response();
    }

    match state.ath.db().delete_user(user_id).await {
        Ok(()) => Redirect::to("/admin/users?success=User+deleted").into_response(),
        Err(AuthError::NotFound) => {
            Redirect::to("/admin/users?error=User+not+found").into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to delete user");
            Redirect::to("/admin/users?error=Failed+to+delete+user").into_response()
        }
    }
}


#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use allowthem_core::{AllowThemBuilder, Email, EmbeddedAuthClient, generate_token, hash_token};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::{Duration, Utc};
    use tower::ServiceExt;

    use crate::config::AppConfig;
    use crate::db::Db;
    use crate::server::AppState;
    use crate::templates::Templates;

    async fn test_state() -> Arc<AppState> {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");

        let ath = AllowThemBuilder::with_pool(db.pool().clone())
            .cookie_secure(false)
            .build()
            .await
            .expect("allowthem build");
        let auth_client = Arc::new(EmbeddedAuthClient::new(ath.clone(), "/login"));

        let config = AppConfig::default();
        let templates = Templates::new(Templates::default_dir());
        AppState::new(config, "sendword.toml", db, templates, ath, auth_client)
    }

    /// Create a test user and return a session cookie value for authenticated requests.
    async fn create_test_session(state: &Arc<AppState>) -> String {
        let email = Email::new("admin@example.com".into()).unwrap();
        let user = state
            .ath
            .db()
            .create_user(email, "password123", None, None)
            .await
            .unwrap();

        let token = generate_token();
        let token_hash = hash_token(&token);
        let expires = Utc::now() + Duration::hours(24);
        state
            .ath
            .db()
            .create_session(user.id, token_hash, None, None, expires)
            .await
            .unwrap();

        // session_cookie returns Set-Cookie value; extract name=value for Cookie header
        let cookie = state.ath.session_cookie(&token);
        cookie.split(';').next().unwrap().to_string()
    }

    fn app(state: Arc<AppState>) -> axum::Router {
        crate::server::router(state, axum::Router::new())
    }

    #[tokio::test]
    async fn list_users_requires_auth() {
        let state = test_state().await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/admin/users")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Should redirect to login
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn list_users_shows_current_user() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/admin/users")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        // user rows include email as the username field
        assert!(html.contains("admin@example.com"));
        assert!(html.contains("YOU"));
    }

    #[tokio::test]
    async fn create_user_redirects_with_success() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/users")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("email=newuser@example.com&password=secret123"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("success="));

        // Verify user was created
        let users = state.ath.db().list_users().await.unwrap();
        assert_eq!(users.len(), 2);
    }

    #[tokio::test]
    async fn create_user_rejects_duplicate() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        // Try to create the admin user again
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/users")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("email=admin@example.com&password=other"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
    }

    #[tokio::test]
    async fn delete_self_is_prevented() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;
        let email = Email::new("admin@example.com".into()).unwrap();
        let admin = state.ath.db().get_user_by_email(&email).await.unwrap();

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/admin/users/{}/delete", admin.id))
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
        assert!(location.contains("yourself"));
    }

    #[tokio::test]
    async fn delete_other_user_succeeds() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        // Create another user to delete
        let other_email = Email::new("other@example.com".into()).unwrap();
        let other = state
            .ath
            .db()
            .create_user(other_email, "password", None, None)
            .await
            .unwrap();

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/admin/users/{}/delete", other.id))
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("success="));

        // Verify user was deleted
        let users = state.ath.db().list_users().await.unwrap();
        assert_eq!(users.len(), 1);
    }

}
