use std::sync::Arc;
use axum::Router;
use axum::routing::get;
use crate::core::AppState;
use crate::user_relationship::user_handler::search_user_by_id;

pub fn create_user_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/users/{user_id}", get(search_user_by_id))
}