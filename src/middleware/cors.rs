//! Which browser origin may talk to this server, and with what.

use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderValue, Method};
use http::header::{CONNECTION, CONTENT_LENGTH, ORIGIN};
use tower_http::cors::CorsLayer;

/// Builds the CORS layer from the configured origin.
///
/// A single exact origin rather than a wildcard: credentials are allowed, and the two are mutually
/// exclusive per the CORS spec — a wildcard would make every authenticated request fail in the
/// browser.
///
/// Panics on an unparseable origin, in line with the other startup-configuration failures. A
/// typo'd `cors_origin` otherwise produces a server that answers every browser request correctly
/// on the wire while the client sees nothing but opaque CORS errors.
pub fn cors_layer(origin: &str) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(
            origin
                .parse::<HeaderValue>()
                .unwrap_or_else(|err| panic!("Invalid CORS origin {origin:?}: {err}")),
        )
        .allow_headers([
            AUTHORIZATION,
            ACCEPT,
            CONTENT_TYPE,
            CONTENT_LENGTH,
            CONNECTION,
            ORIGIN,
        ])
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS, Method::DELETE])
}