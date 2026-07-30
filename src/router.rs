//! The API route tree.
//!
//! Route registration only. Everything wrapped *around* these routes — tracing, CORS, panic
//! handling, authentication, body limits — lives in [`crate::middleware`], including the order it
//! runs in.

use crate::core::AppState;
use crate::messaging::routes::create_messaging_routes;
use crate::middleware;
use crate::rooms::routes::create_room_routes;
use crate::users::routes::create_user_routes;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use std::sync::Arc;

/// Initializes the api routes.
///
/// Performs the startup OIDC discovery while building the middleware stack, and panics if it
/// fails — see [`middleware::apply`].
pub async fn init_router(app_state: AppState) -> Router {
    let public_routing = Router::new()
        .route("/", get(|| async { "Hello, world! I'm your new ISM. 🤗" }))
        .route("/health", get(|| async { (StatusCode::OK, "Healthy").into_response() }), );

    let protected_routing = Router::new().nest(
        "/api/v1", //add new routes here, the /api prefix is applied once via nest
        Router::new()
            .merge(create_room_routes())
            .merge(create_user_routes())
            .merge(create_messaging_routes()),
    );

    // Borrowing the config has to finish before the state is moved into the `Arc`.
    let protected_routing = middleware::apply(protected_routing, &app_state.env).await;
    let protected_routing = protected_routing.with_state(Arc::new(app_state));

    public_routing.merge(protected_routing)
}
