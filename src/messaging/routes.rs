use std::sync::Arc;
use axum::Router;
use axum::routing::{get, post};
use crate::core::AppState;
use crate::messaging::messages::send_message;
use crate::messaging::notifications::{add_notification, poll_for_new_notifications, stream_server_events};

pub fn create_messaging_routes() -> Router<Arc<AppState>> {
    Router::new() //add new routes here
        .route("/api/notify", get(poll_for_new_notifications))
        .route("/api/sse", get(stream_server_events))
        .route("/api/notify", post(add_notification))
        .route("/api/send-msg", post(send_message))
}