use std::collections::HashSet;
use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::{Multipart, Path, Query, State};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use log::error;
use serde::Deserialize;
use uuid::Uuid;
use crate::core::AppState;
use crate::errors::{AppError};
use crate::keycloak::decode::KeycloakToken;
use crate::messaging::model::MessageDTO;
use crate::model::{ChatRoom, ChatRoomWithUserDTO, NewRoom, RoomMember, RoomType, UploadResponse};
use crate::rooms::room_service::RoomService;
use crate::rooms::timeline_service::TimelineService;
use crate::user_relationship::user_service::UserService;
use crate::utils::check_user_in_room;

#[derive(Deserialize, Debug)]
pub struct RoomSearchQueryParam {
    #[serde(rename = "withUser")]
    pub with_user: Uuid
}

#[derive(Deserialize)]
pub struct TimelineQueryParam {
    timestamp: DateTime<Utc>
}

pub async fn handle_scroll_chat_timeline(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>,
    Query(params): Query<TimelineQueryParam>
) -> Result<Json<Vec<MessageDTO>>, AppError> {

    check_user_in_room(&state, &token.subject, &room_id).await?;
    let messages = TimelineService::scroll_chat_timeline(state, room_id, params.timestamp).await?;
    Ok(Json(messages))
}

pub async fn handle_get_users_in_room(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> Result<Json<Vec<RoomMember>>, AppError> {

    check_user_in_room(&state, &token.subject, &room_id).await?;
    let users = RoomService::get_users_in_room(state, room_id).await?;
    Ok(Json(users))
}

pub async fn handle_get_joined_rooms(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>
) -> Result<Json<Vec<ChatRoom>>, AppError> {

    let rooms = RoomService::get_joined_rooms(state, token.subject).await?;
    Ok(Json(rooms))
}

pub async fn handle_get_room_with_details(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> Result<Json<ChatRoomWithUserDTO>, AppError> {

    let room = RoomService::get_room_with_details(state, token.subject, room_id).await?;
    Ok(Json(room))
}

pub async fn mark_room_as_read(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> Result<(), AppError> {
    RoomService::mark_room_as_read(state, token.subject, room_id).await?;
    Ok(())
}

pub async fn handle_create_room(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Json(mut payload): Json<NewRoom>
) -> Result<Json<ChatRoom>, AppError> {

    if !payload.invited_users.contains(&token.subject) {
        return Err(AppError::ValidationError("Sender ID is not in the list of invited users.".to_string()));
    }
    
    
    //filter out all users that have an ignore-relationship with the sender
    let ignored = UserService::get_blocked_users(state.clone(), &token.subject, &payload.invited_users).await?;
    let filter_set: HashSet<_> = ignored.iter().collect();
    payload.invited_users.retain(|uuid| !filter_set.contains(uuid));
    

    match payload.room_type {
        RoomType::Single => {
            if payload.invited_users.len() != 2 {
                return Err(AppError::ValidationError("Personal rooms must have exactly two IDs (sender + one other).".to_string()));
            }
            let other_user = payload.invited_users.iter().find(|&&el| el != token.subject).ok_or_else(|| {
                AppError::ValidationError("Personal rooms must contain another user.".to_string())
            })?;
            let has_active_chat = RoomService::find_existing_single_room(state.clone(), &token.subject, other_user).await?;
            if has_active_chat.is_some() {
                return Err(AppError::ValidationError("User already has an active personal chat.".to_string()));
            }
        }
        RoomType::Group => {
            if payload.invited_users.len() < 2 {
                return Err(AppError::ValidationError("Groups must have more than one user.".to_string()));
            }
        }
    }
    let room = RoomService::create_room(state, token.subject, payload).await?;
    Ok(Json(room))
}

pub async fn handle_get_room_list_item_by_id(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> Result<Json<ChatRoom>, AppError> {
    let room = RoomService::get_room_list_item_by_id(state, token.subject, room_id).await?;
    Ok(Json(room))
}

pub async fn handle_leave_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> Result<(), AppError> {
    RoomService::leave_room(state, token.subject, room_id).await?;
    Ok(())
}

pub async fn handle_invite_to_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path((room_id, user_id)): Path<(Uuid, Uuid)>
) -> Result<(), AppError> {

    let ignored = UserService::get_blocked_users(state.clone(), &token.subject, &vec!(user_id)).await?;
    if ignored.contains(&user_id) {
        return Err(AppError::Blocked("User is blocked.".to_string()));
    }

    RoomService::invite_to_room(state, token.subject, room_id, user_id).await?;
    Ok(())
}


pub async fn handle_search_existing_single_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Query(params): Query<RoomSearchQueryParam>,
) -> Result<Json<Option<Uuid>>, AppError> {
    let result = RoomService::find_existing_single_room(state, &token.subject, &params.with_user).await?;
    Ok(Json(result))
}

pub async fn handle_save_room_image(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>,
    mut multipart: Multipart
) -> Result<Json<UploadResponse>, AppError> {
    check_user_in_room(&state, &token.subject, &room_id).await?;
    let mut image_data: Option<Bytes> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() ==  Some("image") {
                    let data = match field.bytes().await {
                        Ok(data) => data,
                        Err(_) => {
                            return Err(AppError::ValidationError("Error reading the image byte stream.".to_string()))
                        }
                    };
                    image_data = Some(data);
                    break;
                }
            },
            Ok(None) => {
                break; //stream finished
            }
            Err(err) => { //read error
                error!("Bad image upload: {}", err.to_string());
                return Err(AppError::ValidationError("Error reading the image byte stream.".to_string()))
            }
        }
    }

    if let Some(image_data) = image_data {
        let response = RoomService::set_room_image(state, room_id, image_data).await?;
        Ok(Json(response))
    } else {
        Err(AppError::ValidationError("Required field 'image' not found in the upload.".to_string()))
    }
}