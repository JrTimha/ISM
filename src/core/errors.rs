use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Serialize;
use thiserror::Error;
use validator::ValidationErrors;

#[derive(Serialize)]
pub struct ErrorResponse {
    timestamp: String,
    status: u16,
    error: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(rename = "errorCode")]
    error_code: ErrorCode,
}

impl ErrorResponse {
    pub fn new(status: StatusCode, error_code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now().to_rfc3339(),
            status: status.as_u16(),
            error: status.canonical_reason().unwrap_or("Unknown").to_string(),
            message: message.into(),
            path: None,
            error_code,
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[allow(dead_code)]
pub enum ErrorCode {
    // Authentication & Authorization
    InsufficientPermissions,

    // User & Profile Errors
    UserNotFound,

    // Content & Interaction Errors
    RoomNotFound,
    MessageNotFound,
    InvalidContent,
    FileProcessingError,
    ContentNotFound,

    // General API & Validation Errors
    ValidationError,
    ServiceUnavailable,
    UnexpectedError,
}

/// Application-level error type used across handlers and services.
///
/// Variants are split into two groups:
///
/// **Client-facing** – the message is passed through to the HTTP response body.
/// Use these when the caller needs actionable feedback (bad input, missing resource, no permission).
///
/// **Internal** – the full error is logged server-side; only a generic message reaches the client.
/// Use these for infrastructure failures (object_storage, cache, S3) where internal details must not leak.
#[derive(Debug, Error)]
pub enum AppError {
    // ── Client-facing ────────────────────────────────────────────────────────
    /// 400 – invalid or rejected input from the caller.
    #[error("{0}")]
    Validation(String),

    /// 404 – the requested resource does not exist.
    #[error("{0}")]
    NotFound(String),

    /// 403 – the caller is authenticated but lacks the required permission.
    #[error("{0}")]
    Forbidden(String),

    // ── Internal (logged; generic message sent to client) ────────────────────
    /// PostgreSQL / SQLx failure.
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Redis cache failure.
    #[error("Cache error: {0}")]
    Cache(#[from] redis::RedisError),

    /// JSON serialisation / deserialisation failure.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// S3 / MinIO object-storage failure.
    #[error("S3 error: {0}")]
    S3(String),

    /// Any other internal processing failure not covered by the variants above.
    #[error("Processing error: {0}")]
    Processing(String),
}

impl From<ValidationErrors> for AppError {
    fn from(errors: ValidationErrors) -> Self {
        AppError::Validation(errors.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Log every internal error with its full details before the message is sanitised.
        match &self {
            AppError::Database(_)
            | AppError::Cache(_)
            | AppError::Serialization(_)
            | AppError::S3(_)
            | AppError::Processing(_) => tracing::error!("{}", self),
            _ => {}
        }

        let (status, error_code, message) = match self {
            // Client-facing — pass the message through unchanged.
            AppError::Validation(msg) => (StatusCode::BAD_REQUEST, ErrorCode::ValidationError, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, ErrorCode::ContentNotFound, msg),
            AppError::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                ErrorCode::InsufficientPermissions,
                msg,
            ),

            // Internal — return a safe, generic message.
            AppError::Database(_) | AppError::Cache(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::ServiceUnavailable,
                "Internal server error. Please try again later.".to_owned(),
            ),
            AppError::S3(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::FileProcessingError,
                "File operation failed. Please try again later.".to_owned(),
            ),
            AppError::Serialization(_) | AppError::Processing(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::UnexpectedError,
                "An unexpected error occurred.".to_owned(),
            ),
        };

        let body = ErrorResponse::new(status, error_code, message);
        (status, Json(body)).into_response()
    }
}

pub type AppResponse<T> = Result<T, AppError>;
