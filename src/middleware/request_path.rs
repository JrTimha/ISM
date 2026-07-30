//! Stamping the request path onto error response bodies.

use axum::body::to_bytes;
use axum::extract::{MatchedPath, Request};
use axum::http::Uri;
use axum::middleware::Next;
use axum::response::Response;
use http::header::CONTENT_LENGTH;

/// Adds `path` to the JSON body of every error response.
///
/// `ErrorResponse` leaves the field empty because nothing that constructs one — an `AppError` in a
/// handler, an `AuthError` in the middleware — knows the route it was reached through. This runs
/// as the outermost layer so it covers both.
pub async fn inject_request_path(
    matched_path: Option<MatchedPath>,
    uri: Uri,
    req: Request,
    next: Next,
) -> Response {
    let path = matched_path
        .map(|mp| mp.as_str().to_owned())
        .unwrap_or_else(|| uri.path().to_owned());

    let response = next.run(req).await;

    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 64 * 1024).await {
        Ok(b) => b,
        Err(_) => return Response::from_parts(parts, axum::body::Body::empty()),
    };

    if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        if let Some(obj) = json.as_object_mut() {
            obj.insert("path".to_owned(), serde_json::json!(path));
        }
        if let Ok(new_body) = serde_json::to_vec(&json) {
            parts.headers.remove(CONTENT_LENGTH);
            return Response::from_parts(parts, axum::body::Body::from(new_body));
        }
    }

    Response::from_parts(parts, axum::body::Body::from(bytes))
}