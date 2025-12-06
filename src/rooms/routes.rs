use std::sync::Arc;
use axum::Router;
use axum::routing::{get, post};
use crate::core::AppState;
use crate::rooms::handler::{handle_create_room, handle_get_joined_rooms, handle_get_room_list_item_by_id, handle_get_room_with_details, handle_get_users_in_room, handle_invite_to_room, handle_leave_room, handle_save_room_image, handle_scroll_chat_timeline, handle_search_existing_single_room, mark_room_as_read};


pub fn create_room_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/rooms/create-room", post(handle_create_room))
        .route("/api/rooms/{room_id}/users", get(handle_get_users_in_room))
        .route("/api/rooms/{room_id}/detailed", get(handle_get_room_with_details))
        .route("/api/rooms/{room_id}/timeline", get(handle_scroll_chat_timeline))
        .route("/api/rooms/{room_id}/mark-read", post(mark_room_as_read))
        .route("/api/rooms/{room_id}", get(handle_get_room_list_item_by_id))
        .route("/api/rooms/{room_id}/leave", post(handle_leave_room))
        .route("/api/rooms/search", get(handle_search_existing_single_room))
        .route("/api/rooms/{room_id}/invite/{user_id}", post(handle_invite_to_room))
        .route("/api/rooms/{room_id}/upload-img", post(handle_save_room_image))
        .route("/api/rooms", get(handle_get_joined_rooms))
}
