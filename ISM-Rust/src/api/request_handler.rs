use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum_keycloak_auth::decode::KeycloakToken;
use chrono::Utc;
use log::{error};
use uuid::Uuid;
use crate::api::errors::{HttpError};
use crate::api::{AppState, NotificationCache};
use crate::database::{get_message_repository_instance, RoomRepository};
use crate::model::{Message, NewMessage, NewRoom, RoomType};

pub async fn poll_for_new_notifications(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(notifications): Extension<NotificationCache>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    let reader = notifications.read().await;
    if let Some(notes) = reader.get(&id){
        let notification = notes.read().await.clone();
        Json(notification).into_response()
    } else {
        Json::<Vec<String>>(vec![]).into_response()
    }
}

pub async fn scroll_chat_timeline() -> &'static str {
    "Not Implemented"
}

pub async fn send_message(
    Extension(token): Extension<KeycloakToken<String>>,
    Json(payload): Json<NewMessage>
) -> impl IntoResponse {
    let db = get_message_repository_instance().await;
    let id = parse_uuid(&token.subject).unwrap();

    let msg = Message {
        chat_room_id: payload.chat_room_id,
        message_id: Uuid::new_v4(),
        sender_id: id,
        msg_body: payload.msg_body,
        msg_type: payload.msg_type.to_string(),
        created_at: Utc::now(),
    };
    match db.insert_data(msg.clone()).await {
        Ok(_) => {(StatusCode::CREATED, Json(msg)).into_response()},
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
    match state.social_repository.select_all_user_in_room(room_id).await {
        Ok(users) => Json(users).into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
    }
}

pub async fn get_joined_rooms(
    Extension(state): Extension<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    match state.social_repository.get_joined_rooms(id).await {
        Ok(rooms) => Json(rooms).into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
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
