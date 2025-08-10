use std::fmt::Display;
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
    path: String,
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
            path: "unknown".to_string(), //placeholder
            error_code: self.error_code,
        };

        (status, Json(error_response)).into_response()
    }
}