use crate::core::AppState;
use crate::messaging::handler::handle_send_message;
use crate::messaging::notifications::{
    get_latest_notification_events, get_notification_cursor, stream_server_events,
    websocket_server_events,
};
use axum::Router;
use axum::routing::{any, get, post};
use std::sync::Arc;

pub fn create_messaging_routes() -> Router<Arc<AppState>> {
    Router::new() //add new routes here
        .route("/api/notifications", get(get_latest_notification_events))
        .route("/api/notifications/cursor", get(get_notification_cursor))
        .route("/api/sse", get(stream_server_events))
        .route("/api/wss", any(websocket_server_events))
        .route("/api/send-msg", post(handle_send_message))
}
