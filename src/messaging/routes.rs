use std::sync::Arc;
use axum::Router;
use axum::routing::{get, post};
use crate::core::AppState;
use crate::messaging::handler::handle_send_message;
use crate::messaging::notifications::{get_latest_notification_events, stream_server_events};

pub fn create_messaging_routes() -> Router<Arc<AppState>> {
    Router::new() //add new routes here
        .route("/api/notifications", get(get_latest_notification_events))
        .route("/api/sse", get(stream_server_events))
        .route("/api/send-msg", post(handle_send_message))
}