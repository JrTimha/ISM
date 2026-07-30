//! Turning an unwinding handler into an ordinary error response.

use crate::core::errors::{ErrorCode, ErrorResponse};
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::any::Any;
use tower_http::catch_panic::CatchPanicLayer;

/// A plain function pointer rather than the unnameable type of the `panic_response` fn item, so
/// the layer can appear in a return type at all. Pointers are `Copy`, which satisfies the `Clone`
/// bound `ResponseForPanic` carries.
type PanicHandler = fn(Box<dyn Any + Send + 'static>) -> Response;

/// Catches a panic from anything below this layer and answers with a 500.
///
/// Without it an unwinding handler takes the connection down with it: hyper drops the socket, the
/// caller sees a reset rather than a response, and nothing is recorded with the request attached.
///
/// Relies on unwinding: under `panic = "abort"` the process dies before this ever runs.
pub fn catch_panic_layer() -> CatchPanicLayer<PanicHandler> {
    CatchPanicLayer::custom(panic_response as PanicHandler)
}

/// Renders a caught panic as the same error envelope every other failure uses.
fn panic_response(err: Box<dyn Any + Send + 'static>) -> Response {
    // `panic!` boxes a `&'static str` for a literal and a `String` for a formatted message.
    // Anything else — `panic_any` with a custom type — carries no text we can render.
    let details = if let Some(msg) = err.downcast_ref::<&'static str>() {
        *msg
    } else if let Some(msg) = err.downcast_ref::<String>() {
        msg.as_str()
    } else {
        "<panic payload was not a string>"
    };

    // The default panic hook has already written the payload and its source location to stderr.
    // What it cannot know is which request caused it, which is exactly what the surrounding span
    // adds here.
    tracing::error!(error.kind = "panic", panic = %details, "Handler panicked");

    // Deliberately the same opaque message the internal `AppError` variants return: a panic
    // message is as likely to leak internals as a database error is.
    let body = ErrorResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::UnexpectedError,
        "An unexpected error occurred.",
    );
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.expect("readable body");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    }

    /// A caught panic has to be indistinguishable from any other internal error on the wire: same
    /// envelope, same opaque message, and nothing from the payload leaked into it. Panic messages
    /// routinely contain table names, ids and file paths.
    #[tokio::test]
    async fn panic_response_matches_the_internal_error_envelope() {
        let secret = "participant row for 3f2b vanished mid-write";
        let response = panic_response(Box::new(secret.to_owned()));

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = body_of(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).expect("json body");

        assert_eq!(json["status"], 500);
        assert_eq!(json["errorCode"], "UNEXPECTED_ERROR");
        assert_eq!(json["message"], "An unexpected error occurred.");
        // `path` is filled in by `inject_request_path`, which sits outside this layer.
        assert!(json.get("path").is_none());
        assert!(!body.contains(secret), "panic payload leaked into the response: {body}");
    }

    /// `panic!` with a literal boxes a `&'static str` rather than a `String`, so it takes the other
    /// downcast branch.
    #[tokio::test]
    async fn panic_response_handles_a_static_str_payload() {
        let response = panic_response(Box::new("unreachable branch reached"));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body_of(response).await.contains("unreachable"));
    }

    /// `panic_any` can carry an arbitrary type. The handler must still answer rather than panic a
    /// second time while trying to describe the first one.
    #[tokio::test]
    async fn panic_response_handles_a_non_string_payload() {
        let response = panic_response(Box::new(42_u8));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let json: serde_json::Value =
            serde_json::from_str(&body_of(response).await).expect("json body");
        assert_eq!(json["errorCode"], "UNEXPECTED_ERROR");
    }
}