use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("migration error: {0}")]
    Migration(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("validation: {0}")]
    Validation(String),
}

pub type DbResult<T> = Result<T, DbError>;

// --- AppError: Axum handler error type ---

pub enum AppError {
    NotFound(&'static str),
    Internal(eyre::Report),
}

impl AppError {
    pub fn not_found(resource: &'static str) -> Self {
        Self::NotFound(resource)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound(resource) => {
                (StatusCode::NOT_FOUND, format!("{resource} not found")).into_response()
            }
            Self::Internal(report) => {
                tracing::error!(error = %report, "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
            }
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<eyre::Report>,
{
    fn from(err: E) -> Self {
        Self::Internal(err.into())
    }
}
