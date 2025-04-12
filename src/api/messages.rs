use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::State;
use axum::response::IntoResponse;
use chrono::Utc;
use http::StatusCode;
use log::error;
use uuid::Uuid;
use crate::api::errors::HttpError;
use crate::api::utils::parse_uuid;
use crate::broadcast::{BroadcastChannel, Notification, NotificationEvent};
use crate::core::AppState;
use crate::database::RoomRepository;
use crate::keycloak::decode::KeycloakToken;
use crate::model::{Message, NewMessage};


pub async fn send_message(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<NewMessage>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();

    let mut users = match state.room_repository.select_room_participants_ids(&payload.chat_room_id).await {
        Ok(ids) => ids,
        Err(error) => {
            error!("{}", error.to_string());
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    if !users.contains(&id) {
        return HttpError::unauthorized("Room not found or access denied.").into_response();
    }

    users.retain(|&user| {
        user != id
    });

    let msg = Message {
        chat_room_id: payload.chat_room_id,
        message_id: Uuid::new_v4(),
        sender_id: id,
        msg_body: payload.msg_body,
        msg_type: payload.msg_type.to_string(),
        created_at: Utc::now(),
    };
    let json = match serde_json::to_value(&msg) {
        Ok(json) => json,
        Err(_) => return StatusCode::BAD_REQUEST.into_response()
    };

    if let Err(err) = state.message_repository.insert_data(msg.clone()).await {
        error!("{}", err.to_string());
        return HttpError::bad_request("Can't safe message in timeline").into_response();
    }
    let displayed = match state.room_repository.update_last_room_message(&payload.chat_room_id, &msg).await {
        Ok(displayed) => displayed,
        Err(error) => {
            error!("{}", error);
            return HttpError::bad_request("Can't update the state of the chat room.").into_response();
        }
    };
    if let Err(err) = state.room_repository.update_user_read_status(&payload.chat_room_id, &msg.sender_id).await {
        error!("{}", err);
        return HttpError::bad_request("Can't update user read status.").into_response();
    }

    let note = Notification {
        notification_event: NotificationEvent::ChatMessage,
        body: json,
        created_at: msg.created_at,
        display_value: Option::from(displayed)
    };
    BroadcastChannel::get().send_event_to_all(users, note).await;
    (StatusCode::CREATED, Json(msg)).into_response()
}