use std::error::Error;
use std::fmt;
use std::fmt::{Display, Formatter};
use axum::http::StatusCode;
use axum::Json;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Serialize;

#[derive(Serialize)]
pub struct ErrorResponse {
    timestamp: String,
    status: u16,
    error: String,
    message: String,
    path: Option<String>,
    #[serde(rename = "errorCode")]
    error_code: ErrorCode,
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

impl ErrorCode {
    fn to_str(&self) -> String {
        match self {
            ErrorCode::UnexpectedError => "Server Error. Please try again later".to_string(),
            ErrorCode::UserNotFound => "User not found.".to_string(),
            ErrorCode::InsufficientPermissions => "You are not allowed to perform this action".to_string(),
            _ => format!("{:?}", self),
        }
    }
}

impl Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_str().to_owned())
    }
}

#[derive(Debug)]
pub struct HttpError {
    pub status_code: StatusCode,
    pub error_code: ErrorCode,
    pub message: String,
}

impl HttpError {

    pub fn new(status_code: StatusCode, error_code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status_code,
            error_code,
            message: message.into(),
        }
    }

    pub fn bad_request(error_code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status_code: StatusCode::BAD_REQUEST,
            error_code,
            message: message.into(),
        }
    }


}


impl IntoResponse for HttpError {
    fn into_response(self) -> Response {

        tracing::error!("An error occurred: status={}, code={:?}, msg='{}'", self.status_code, self.error_code, self.message);

        let status = self.status_code;

        let error_response = ErrorResponse {
            timestamp: Utc::now().to_rfc3339(),
            status: status.as_u16(),
            error: status.canonical_reason().unwrap_or("Unknown Status").to_string(),
            message: self.message.clone(),
            path: None,
            error_code: self.error_code,
        };

        (status, Json(error_response)).into_response()
    }
}

pub enum AppError {
    /// Ein Fehler, der von einer ungültigen Anfrage des Clients herrührt.
    ValidationError(String),

    /// Ein angeforderter Datensatz wurde nicht gefunden.
    NotFound(String),

    /// Ein Fehler, der aus der Datenbank kommt. Wir verpacken den ursprünglichen Fehler.
    /// `Box<dyn Error + Send + Sync>` ist der Standardweg in Rust, um einen beliebigen Fehler zu speichern.
    DatabaseError(Box<dyn Error + Send + Sync>),

    /// Ein interner Fehler bei der Verarbeitung, z.B. beim Kodieren/Dekodieren.
    ProcessingError(String),

    Blocked(String),
}

impl fmt::Debug for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValidationError(msg) => write!(f, "ValidationError: {}", msg),
            Self::NotFound(msg) => write!(f, "NotFound: {}", msg),
            Self::DatabaseError(err) => write!(f, "DatabaseError: {}", err),
            Self::ProcessingError(msg) => write!(f, "ProcessingError: {}", msg),
            Self::Blocked(msg) => write!(f, "Blocked: {}", msg),
        }
    }
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AppError::ValidationError(msg) => write!(f, "Invalid input: {}", msg),
            AppError::NotFound(msg) => write!(f, "Entity not found: {}", msg),
            AppError::DatabaseError(err) => write!(f, "Ein Datenbankfehler ist aufgetreten: {}", err),
            AppError::ProcessingError(msg) => write!(f, "Ein Verarbeitungsfehler ist aufgetreten: {}", msg),
            AppError::Blocked(msg) => write!(f, "Blocked: {}", msg),
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> AppError {
        AppError::DatabaseError(Box::new(err))
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            AppError::DatabaseError(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {

        let http_error = match self {
            AppError::ValidationError(msg) => {
                HttpError::new(StatusCode::BAD_REQUEST, ErrorCode::ValidationError, msg)
            }
            AppError::NotFound(msg) => {
                HttpError::new(StatusCode::NOT_FOUND, ErrorCode::ContentNotFound, msg)
            }
            AppError::DatabaseError(internal_err) => {
                tracing::error!("Database error: {:?}", internal_err);
                HttpError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::ServiceUnavailable,
                    "Internal service outage."
                )
            }
            AppError::ProcessingError(msg) => {
                tracing::error!("Intern processing error: {}", msg);
                HttpError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::UnexpectedError,
                    "Unexpected server error processing."
                )
            }
            AppError::Blocked(msg) => {
                HttpError::new(
                    StatusCode::FORBIDDEN,
                    ErrorCode::InsufficientPermissions,
                    msg
                )
            }
        };

        http_error.into_response()
    }
}