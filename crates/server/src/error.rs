use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("Not signed in")]
    Unauthorized,
    #[error("You don't have permission to do that")]
    Forbidden,
    #[error("Not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type ApiResult<T> = Result<Json<T>, AppError>;

pub fn bad(msg: impl Into<String>) -> AppError {
    AppError::BadRequest(msg.into())
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::RowNotFound => AppError::NotFound,
            sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => {
                AppError::Conflict(format!("Already exists ({})", d.constraint().unwrap_or("duplicate")))
            }
            sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => {
                AppError::BadRequest(format!("Refers to a record that doesn't exist ({})", d.constraint().unwrap_or("foreign key")))
            }
            sqlx::Error::Database(d) if d.code().as_deref() == Some("23514") => {
                AppError::BadRequest(format!("Invalid value ({})", d.constraint().unwrap_or("check")))
            }
            _ => AppError::Internal(e.into()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let msg = match &self {
            AppError::Internal(_) => "Something went wrong on the server".to_string(),
            other => other.to_string(),
        };
        (status, Json(json!({ "success": false, "error": msg }))).into_response()
    }
}
