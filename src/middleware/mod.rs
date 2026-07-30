//! The middleware stack wrapped around the protected routes.
//!
//! `router.rs` owns the route tree; this module owns everything that runs before a handler is
//! reached and after its response comes back. Each layer is configured in its own file next to the
//! reasoning for that configuration. The one thing that cannot be split up — the order they run in
//! — is [`apply`].
//!
//! | File | Responsibility |
//! |---|---|
//! | `request_path.rs` | stamps `path` onto error response bodies |
//! | `trace.rs` | the `HTTP_REQUEST` span every log line from a request hangs off |
//! | `cors.rs` | which browser origin may talk to this server |
//! | `catch_panic.rs` | turns an unwinding handler into a 500 instead of a dropped connection |
//! | `auth.rs` | wires ISM's config into the Keycloak layer; startup OIDC discovery |

mod auth;
mod catch_panic;
mod cors;
mod request_path;
mod trace;

use crate::core::{AppState, ISMConfig};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use std::sync::Arc;
use tower::ServiceBuilder;

/// Largest request body a protected route will buffer.
///
/// Applies to every route, not just the upload endpoint — a handler that never reads a body would
/// otherwise still let a caller push megabytes into memory before rejecting them.
const MAX_BODY_SIZE: usize = 5 * 1024 * 1024;

/// Wraps `router` in the full middleware stack.
///
/// Listed outermost first, which is `ServiceBuilder`'s own direction:
///
/// | # | Layer | Why it sits here |
/// |---|---|---|
/// | 1 | `inject_request_path` | outermost, so it stamps `path` onto every error body — including the ones the auth layer produces before a handler ever runs |
/// | 2 | `TraceLayer` | must observe the final status, so it sits above everything that can turn a request into an error |
/// | 3 | `CorsLayer` | has to wrap error responses too, or a browser cannot read the body of a 401 or a 500 |
/// | 4 | `CatchPanicLayer` | inside CORS so its 500 carries the headers; outside auth so a panic during token validation is covered as well |
/// | 5 | `KeycloakAuthLayer` | everything below it runs with a validated token |
/// | 6 | `DefaultBodyLimit` | innermost, so only callers that got past auth can make the server buffer a body |
///
/// Performs the startup OIDC discovery on the way, and panics if it fails — see
/// [`auth::auth_layer`].
pub async fn apply(router: Router<Arc<AppState>>, config: &ISMConfig) -> Router<Arc<AppState>> {
    router.layer(
        ServiceBuilder::new()
            .layer(axum::middleware::from_fn(request_path::inject_request_path))
            .layer(trace::http_trace_layer())
            .layer(cors::cors_layer(&config.cors_origin))
            .layer(catch_panic::catch_panic_layer())
            .layer(auth::auth_layer(config.token_issuer.clone()).await)
            .layer(DefaultBodyLimit::max(MAX_BODY_SIZE)),
    )
}
