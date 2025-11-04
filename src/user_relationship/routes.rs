use std::sync::Arc;
use axum::Router;
use axum::routing::{delete, get, post};
use crate::core::AppState;
use crate::user_relationship::handler::{handle_accept_friend_request, handle_add_friend, handle_get_friends, handle_get_open_friend_requests, handle_ignore_user, handle_reject_friend_request, handle_remove_friend, handle_search_user_by_id, handle_search_user_by_name, handle_undo_ignore_user};

pub fn create_user_routes() -> Router<Arc<AppState>> {

    Router::new()
        .route("/api/users/{user_id}", get(handle_search_user_by_id))
        .route("/api/users/search", get(handle_search_user_by_name))
        .route("/api/users/friends/requests", get(handle_get_open_friend_requests))
        .route("/api/users/friends", get(handle_get_friends))
        .route("/api/users/friends/add/{user_id}", post(handle_add_friend))
        .route("/api/users/friends/accept-request/{sender_id}", post(handle_accept_friend_request))
        .route("/api/users/friends/reject-request/{sender_id}", delete(handle_reject_friend_request))
        .route("/api/users/friends/{friend_id}", delete(handle_remove_friend))
        .route("/api/users/ignore/{user_id}", post(handle_ignore_user))
        .route("/api/users/ignore/{user_id}", delete(handle_undo_ignore_user))


}