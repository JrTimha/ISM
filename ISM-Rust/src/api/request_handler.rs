use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum_keycloak_auth::decode::KeycloakToken;
use chrono::{DateTime, Utc};
use log::{error};
use serde::Deserialize;
use uuid::Uuid;
use crate::api::errors::{HttpError};
use crate::api::{AppState, Notification, NotificationEvent};
use crate::api::notification::CacheService;
use crate::database::{get_message_repository_instance, RoomRepository};
use crate::model::{ChatRoomDTO, Message, NewMessage, NewRoom, RoomType};

pub async fn poll_for_new_notifications(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(notifications): Extension<Arc<CacheService>>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    if let Some(notifications) = notifications.get_notifications(id).await {
        Json(notifications).into_response()
    } else {
        Json::<Vec<String>>(vec![]).into_response()
    }
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    timestamp: DateTime<Utc>
}

pub async fn scroll_chat_timeline(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(state): Extension<Arc<AppState>>,
    Path(room_id): Path<Uuid>,
    Query(params): Query<TimelineQuery>
) -> impl IntoResponse {
    let db = get_message_repository_instance().await;
    let id = parse_uuid(&token.subject).unwrap();
    if let Err(err) = check_user_in_room(&state, &id, &room_id).await {
        return err.into_response();
    }
    match db.fetch_data(params.timestamp, room_id).await {
        Ok(data) => {
            Json(data).into_response()
        },
        Err(err) => {
            error!("{}", err.to_string());
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}


pub async fn send_message(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(state): Extension<Arc<AppState>>,
    Extension(notifications): Extension<Arc<CacheService>>,
    Json(payload): Json<NewMessage>
) -> impl IntoResponse {
    let db = get_message_repository_instance().await;
    let id = parse_uuid(&token.subject).unwrap();

    let mut users = match state.social_repository.select_room_participants_ids(&payload.chat_room_id).await {
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

    match db.insert_data(msg.clone()).await {
        Ok(_) => {
            if let _error = state.social_repository.update_last_room_message(&payload.chat_room_id).await {
                return HttpError::bad_request("Can't write chat room.").into_response();
            }
            let note = Notification {
                notification_event: NotificationEvent::ChatMessage,
                body: json,
                created_at: msg.created_at,
            };
            notifications.add_notifications_to_all(users, note).await;
            (StatusCode::CREATED, Json(msg)).into_response()
        },
        Err(err) => {
            error!("{}", err.to_string());
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn get_users_in_room(
    Extension(state): Extension<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    match state.social_repository.select_all_user_in_room(&room_id).await {
        Ok(users) => Json(users).into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
    }
}

pub async fn get_joined_rooms(
    Extension(state): Extension<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    match state.social_repository.get_joined_rooms(&id).await {
        Ok(rooms) => Json(rooms).into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
    }
}

pub async fn get_room_with_details(
    Extension(state): Extension<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    if let Err(err) = check_user_in_room(&state, &id, &room_id).await {
        return err.into_response();
    }

    let res = tokio::try_join!( //executing 2 queries async
        state.social_repository.select_room(&room_id),
        state.social_repository.select_all_user_in_room(&room_id)
    );

    match res {
        Ok((room, user)) => {
            let room_details = ChatRoomDTO {
                id: room.id,
                room_type: room.room_type,
                room_name: room.room_name,
                created_at: room.created_at,
                users: user,
            };
            Json(room_details).into_response()
        }
        Err(err) => {
            HttpError::bad_request(err.to_string()).into_response()
        }
    }

}

pub async fn mark_room_as_read(
    Extension(state): Extension<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    match state.social_repository.update_user_read_status(&room_id, &id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(_) => {
            HttpError::bad_request("Can't update user read status.").into_response()
        }
    }
}


pub async fn create_room(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<NewRoom>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();

    if !payload.invited_users.contains(&id) {
        return HttpError::bad_request("Sender ID is not in the list of invited users.".to_string()).into_response();
    }

    match payload.room_type {
        RoomType::Single => {
            if payload.invited_users.len() != 2 {
                return HttpError::bad_request("Personal rooms must have exactly two IDs (sender + one other).".to_string()).into_response();
            }
        }
        RoomType::Group => {
            if payload.invited_users.len() <= 2 {
                return HttpError::bad_request("Groups must have more than two users.".to_string()).into_response();
            }
        }
    }

    match state.social_repository.insert_room(payload).await {
        Ok(room) => Json(room).into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
    }
}


fn parse_uuid(subject: &str) -> Result<Uuid, HttpError> {
    Uuid::try_parse(subject).map_err(|_| HttpError::bad_request("Invalid token subject".to_string()))
}

async fn check_user_in_room(
    state: &Arc<AppState>,
    user_id: &Uuid,
    room_id: &Uuid,
) -> Result<(), HttpError> {
    let is_in = state
        .social_repository
        .is_user_in_room(user_id, room_id)
        .await
        .map_err(|_| HttpError::bad_request("Failed to check room access."))?;

    if is_in {
        Ok(())
    } else {
        Err(HttpError::unauthorized("Room not found or access denied."))
    }
}