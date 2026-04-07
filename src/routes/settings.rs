use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Form;
use axum::Router;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::user;
use crate::server::AppState;
use crate::templates::context;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/settings/users", get(list_users).post(create_user))
        .route("/settings/users/{id}/delete", axum::routing::post(delete_user))
        .route("/settings/password", get(password_page).post(change_password))
}

// --- Query params for flash messages ---

#[derive(Deserialize)]
struct FlashParams {
    success: Option<String>,
    error: Option<String>,
}

// --- GET /settings/users ---

async fn list_users(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let pool = state.db.pool();
    let all_users = user::list(pool).await?;

    let user_rows: Vec<_> = all_users
        .iter()
        .map(|u| {
            context! {
                id => u.id,
                username => u.username,
                created_at => u.created_at,
                is_self => u.id == auth.user_id,
            }
        })
        .collect();

    let html = state.templates.render(
        "users.html",
        context! {
            users => user_rows,
            success => flash.success,
            error => flash.error,
            username => auth.username,
            nav_active => "settings",
        },
    )?;
    Ok(Html(html))
}

// --- POST /settings/users ---

#[derive(Deserialize)]
struct CreateUserForm {
    username: String,
    password: String,
}

async fn create_user(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreateUserForm>,
) -> Response {
    let pool = state.db.pool();

    if form.password.is_empty() {
        return Redirect::to("/settings/users?error=Password+cannot+be+empty").into_response();
    }

    match user::create(pool, &form.username, &form.password).await {
        Ok(created) => {
            let msg = format!("User '{}' created", created.username);
            let encoded = urlencoding::encode(&msg);
            Redirect::to(&format!("/settings/users?success={encoded}")).into_response()
        }
        Err(crate::error::DbError::Validation(msg)) => {
            let encoded = urlencoding::encode(&msg);
            Redirect::to(&format!("/settings/users?error={encoded}")).into_response()
        }
        Err(crate::error::DbError::Conflict(msg)) => {
            let encoded = urlencoding::encode(&msg);
            Redirect::to(&format!("/settings/users?error={encoded}")).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to create user");
            Redirect::to("/settings/users?error=Failed+to+create+user").into_response()
        }
    }
}

// --- POST /settings/users/:id/delete ---

async fn delete_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    // Prevent self-deletion
    if id == auth.user_id {
        return Redirect::to("/settings/users?error=Cannot+delete+yourself").into_response();
    }

    let pool = state.db.pool();
    match user::delete(pool, &id).await {
        Ok(()) => Redirect::to("/settings/users?success=User+deleted").into_response(),
        Err(crate::error::DbError::NotFound(_)) => {
            Redirect::to("/settings/users?error=User+not+found").into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to delete user");
            Redirect::to("/settings/users?error=Failed+to+delete+user").into_response()
        }
    }
}

// --- GET /settings/password ---

async fn password_page(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let html = state.templates.render(
        "password.html",
        context! {
            success => flash.success,
            error => flash.error,
            username => auth.username,
            nav_active => "settings",
        },
    )?;
    Ok(Html(html))
}

// --- POST /settings/password ---

#[derive(Deserialize)]
struct ChangePasswordForm {
    current_password: String,
    new_password: String,
    confirm_password: String,
}

async fn change_password(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<ChangePasswordForm>,
) -> Response {
    let pool = state.db.pool();

    // Validate new password matches confirmation
    if form.new_password != form.confirm_password {
        return Redirect::to("/settings/password?error=New+passwords+do+not+match")
            .into_response();
    }

    if form.new_password.is_empty() {
        return Redirect::to("/settings/password?error=New+password+cannot+be+empty")
            .into_response();
    }

    // Fetch current user to verify password
    let current_user = match user::get_by_id(pool, &auth.user_id).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch user for password change");
            return Redirect::to("/settings/password?error=Failed+to+change+password")
                .into_response();
        }
    };

    // Verify current password
    if !user::verify_password(&form.current_password, &current_user.password_hash) {
        return Redirect::to("/settings/password?error=Current+password+is+incorrect")
            .into_response();
    }

    // Update password
    match user::update_password(pool, &auth.user_id, &form.new_password).await {
        Ok(()) => {
            Redirect::to("/settings/password?success=Password+updated+successfully")
                .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to update password");
            Redirect::to("/settings/password?error=Failed+to+change+password").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::db::Db;
    use crate::templates::Templates;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn test_state() -> Arc<AppState> {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        let config = AppConfig::default();
        let templates = Templates::new(Templates::default_dir());
        AppState::new(config, "sendword.toml", db, templates)
    }

    /// Create a test user and return a session cookie value for authenticated requests.
    async fn create_test_session(state: &Arc<AppState>) -> String {
        let pool = state.db.pool();
        let u = user::create(pool, "admin", "password123").await.unwrap();
        let session_lifetime = state.config.load().auth.session_lifetime;
        let sess = crate::models::session::create(pool, &u.id, session_lifetime)
            .await
            .unwrap();
        format!("sendword_session={}", sess.id)
    }

    fn app(state: Arc<AppState>) -> Router {
        crate::server::router(state)
    }

    #[tokio::test]
    async fn list_users_requires_auth() {
        let state = test_state().await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/settings/users")
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
                    .uri("/settings/users")
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
        assert!(html.contains("admin"));
        assert!(html.contains("(you)"));
    }

    #[tokio::test]
    async fn create_user_redirects_with_success() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/settings/users")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("username=newuser&password=secret123"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("success="));

        // Verify user was created
        let users = user::list(state.db.pool()).await.unwrap();
        assert_eq!(users.len(), 2);
    }

    #[tokio::test]
    async fn create_user_rejects_duplicate() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        // Try to create "admin" again
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/settings/users")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("username=admin&password=other"))
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
        let admin = user::get_by_username(state.db.pool(), "admin")
            .await
            .unwrap();

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/settings/users/{}/delete", admin.id))
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
        let other = user::create(state.db.pool(), "other-user", "password")
            .await
            .unwrap();

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/settings/users/{}/delete", other.id))
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
        let users = user::list(state.db.pool()).await.unwrap();
        assert_eq!(users.len(), 1);
    }

    #[tokio::test]
    async fn password_page_requires_auth() {
        let state = test_state().await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/settings/password")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn change_password_validates_current() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/settings/password")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "current_password=wrong&new_password=newpass&confirm_password=newpass",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
        assert!(location.contains("incorrect"));
    }

    #[tokio::test]
    async fn change_password_rejects_mismatch() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/settings/password")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "current_password=password123&new_password=new1&confirm_password=new2",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
        assert!(location.contains("match"));
    }

    #[tokio::test]
    async fn change_password_succeeds() {
        let state = test_state().await;
        let cookie = create_test_session(&state).await;

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/settings/password")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "current_password=password123&new_password=newpassword&confirm_password=newpassword",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("success="));

        // Verify new password works
        let admin = user::get_by_username(state.db.pool(), "admin")
            .await
            .unwrap();
        assert!(user::verify_password("newpassword", &admin.password_hash));
        assert!(!user::verify_password("password123", &admin.password_hash));
    }
}
