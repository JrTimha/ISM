use std::str::FromStr;
use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::State;
use axum::response::IntoResponse;
use chrono::Utc;
use http::{StatusCode};
use log::error;
use uuid::Uuid;
use crate::api::errors::{ErrorCode, HttpError};
use crate::api::timeline::msg_to_dto;
use crate::api::utils::parse_uuid;
use crate::broadcast::{BroadcastChannel, Notification};
use crate::broadcast::NotificationEvent::ChatMessage;
use crate::core::AppState;
use crate::keycloak::decode::KeycloakToken;
use crate::model::{Message, MessageBody, MsgType, NewMessage, NewMessageBody, NewReplyBody, RepliedMessageDetails, ReplyBody};


pub async fn send_message(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<NewMessage>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();

    //validate if the user is in the room
    let users = match state.room_repository.select_room_participants_ids(&payload.chat_room_id).await {
        Ok(ids) => ids,
        Err(error) => {
            error!("{}", error.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't fetch room participants.").into_response();
        }
    };
    if !users.contains(&id) {
        return HttpError::new(StatusCode::UNAUTHORIZED, ErrorCode::InsufficientPermissions, "Room not found or access denied.").into_response();
    }


    let msg_body = match payload.msg_body.clone() {
        NewMessageBody::Text(text) => {
            MessageBody::Text(text)
        }
        NewMessageBody::Media(media) => {
            MessageBody::Media(media)
        }
        NewMessageBody::Reply(reply) => {
            let reply = match handle_reply_message(&reply, &state, &payload.chat_room_id).await {
                Ok(reply) => reply,
                Err(err) => {
                    error!("{}", err.to_string());
                    return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't handle reply message.").into_response();
                }
            };
            MessageBody::Reply(reply)
        }
    };
    
    let msg = match Message::new(payload.chat_room_id, id, msg_body) {
        Ok(message) => message,
        Err(err) => {
            error!("{}", err.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't serialize message.").into_response();
        }
    };
    
    
    if let Err(err) = state.message_repository.insert_data(msg.clone()).await {
        error!("{}", err.to_string());
        return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't safe message in timeline.").into_response();
    }
    
    let mut tx = state.room_repository.start_transaction().await.unwrap();
    let displayed = match state.room_repository.update_last_room_message(&mut *tx, &payload.chat_room_id, &msg.sender_id, generate_room_preview_text(&payload)).await {
        Ok(displayed) => displayed,
        Err(error) => {
            error!("{}", error);
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't update the state of the chat room.").into_response();
        }
    };
    if let Err(err) = state.room_repository.update_user_read_status(&mut *tx, &payload.chat_room_id, &msg.sender_id).await {
        error!("{}", err);
        return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't update user read status.").into_response();
    }
    tx.commit().await.unwrap();

    let mapped_msg = match msg_to_dto(msg) {
        Ok(msg) => msg,
        Err(err) => {
            return HttpError::bad_request(ErrorCode::UnexpectedError,format!("Can't serialize message: {}", err)).into_response();
        }
    };
    
    BroadcastChannel::get().send_event_to_all(
        users,
        Notification {
            body: ChatMessage {message: mapped_msg.clone(), display_value: displayed },
            created_at: Utc::now()
        }
    ).await;
    (StatusCode::CREATED, Json(mapped_msg)).into_response()
}

async fn handle_reply_message(msg: &NewReplyBody, state: &Arc<AppState>, room_id: &Uuid) -> Result<ReplyBody, Box<dyn std::error::Error>> {
    let replied_to = state.message_repository.fetch_specific_message(&msg.reply_msg_id, room_id, &msg.reply_created_at).await?;

    let replied_body: MessageBody = serde_json::from_str(&replied_to.msg_body)?;

    let details = match replied_body {
        MessageBody::Text(text) => {
            RepliedMessageDetails::Text(text)
        }
        MessageBody::Media(media) => {
            RepliedMessageDetails::Media(media)
        }
        MessageBody::Reply(reply) => {
            RepliedMessageDetails::Reply {reply_text: reply.reply_text}
        }
        _ => {
            return Err(Box::from("Unknown Reply body"))
        }
    };

    let new_body = ReplyBody {
        reply_msg_id: replied_to.message_id,
        reply_sender_id: replied_to.sender_id,
        reply_msg_type: MsgType::from_str(&replied_to.msg_type)?,
        reply_created_at: replied_to.created_at,
        reply_msg_details: details,
        reply_text: msg.reply_text.clone(),
    };
    Ok(new_body)
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