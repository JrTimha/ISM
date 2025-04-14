use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::State;
use axum::response::IntoResponse;
use chrono::Utc;
use http::StatusCode;
use log::error;
use uuid::Uuid;
use crate::api::errors::HttpError;
use crate::api::timeline::msg_to_dto;
use crate::api::utils::parse_uuid;
use crate::broadcast::{BroadcastChannel, Notification, NotificationEvent};
use crate::core::AppState;
use crate::database::RoomRepository;
use crate::keycloak::decode::KeycloakToken;
use crate::model::{Message, NewMessage, NewMessageBody};


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

    let msg = Message {
        chat_room_id: payload.chat_room_id,
        message_id: Uuid::new_v4(),
        sender_id: id,
        msg_body: serde_json::to_string(&payload.msg_body).unwrap(),
        msg_type: payload.msg_type.to_string(),
        created_at: Utc::now(),
    };


    if let Err(err) = state.message_repository.insert_data(msg.clone()).await {
        error!("{}", err.to_string());
        return HttpError::bad_request("Can't safe message in timeline").into_response();
    }
    let displayed = match state.room_repository.update_last_room_message(&payload.chat_room_id, &msg.sender_id, generate_room_preview_text(&payload)).await {
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


    let mapped_msg = match msg_to_dto(msg.clone()) {
        Ok(msg) => msg,
        Err(err) => {
            return HttpError::bad_request(format!("Can't serialize message: {}", err)).into_response()
        }
    };
    let json = match serde_json::to_value(&mapped_msg) {
        Ok(json) => json,
        Err(_) => return HttpError::bad_request("Can't serialize message").into_response()
    };

    let note = Notification {
        notification_event: NotificationEvent::ChatMessage,
        body: json,
        created_at: mapped_msg.created_at,
        display_value: Option::from(displayed)
    };
    BroadcastChannel::get().send_event_to_all(users, note).await;
    (StatusCode::CREATED, Json(mapped_msg)).into_response()
}

fn generate_room_preview_text(msg: &NewMessage) -> String {
    match &msg.msg_body {
        NewMessageBody::Text(body) => {
            format!(": {}", body.text)
        }
        NewMessageBody::Media(_) => {
            String::from(" hat etwas geteilt.")
        }
        NewMessageBody::Reply(_) => {
            String::from(" hat auf eine Nachricht geantwortet.")
        }
    }
}