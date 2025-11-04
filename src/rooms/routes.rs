use std::sync::Arc;
use axum::Router;
use axum::routing::{get, post};
use crate::core::AppState;
use crate::rooms::rooms::{create_room, get_joined_rooms, get_room_list_item_by_id, get_room_with_details, get_users_in_room, invite_to_room, leave_room, mark_room_as_read, save_room_image, search_existing_single_room};
use crate::rooms::timeline::scroll_chat_timeline;

pub fn create_room_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/rooms/create-room", post(create_room))
        .route("/api/rooms/{room_id}/users", get(get_users_in_room))
        .route("/api/rooms/{room_id}/detailed", get(get_room_with_details))
        .route("/api/rooms/{room_id}/timeline", get(scroll_chat_timeline))
        .route("/api/rooms/{room_id}/mark-read", post(mark_room_as_read))
        .route("/api/rooms/{room_id}", get(get_room_list_item_by_id))
        .route("/api/rooms/{room_id}/leave", post(leave_room))
        .route("/api/rooms/search", get(search_existing_single_room))
        .route("/api/rooms/{room_id}/invite/{user_id}", post(invite_to_room))
        .route("/api/rooms/{room_id}/upload-img", post(save_room_image))
        .route("/api/rooms", get(get_joined_rooms))
}
