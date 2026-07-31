use crate::auth::CurrentUser;
use crate::core::cursor::{CursorResults, clamp_page_size, decode_cursor};
use crate::core::errors::AppError;
use crate::messaging::model::TimelinePage;
use crate::rooms::model::UploadResponse;
use crate::rooms::room::{ChatRoomDto, ChatRoomWithUserDTO, NewRoom, RoomPaginationCursor};
use crate::rooms::room_member::RoomMember;
use crate::rooms::service::{ShareTarget, ShareTargetCursor};
use crate::rooms::{RoomService, ShareService, TimelineService};
use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;
use validator::Validate;

#[derive(Deserialize, Debug)]
pub struct RoomSearchQueryParam {
    #[serde(rename = "withUser")]
    pub with_user: Uuid,
}

#[derive(Deserialize, Debug)]
pub struct RoomListQueryParams {
    /// Optional case-insensitive name filter (other user for single rooms, room name for groups).
    pub name: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct TimelineQueryParam {
    timestamp: DateTime<Utc>,
}

pub async fn handle_scroll_chat_timeline(
    user: CurrentUser,
    State(timeline): State<TimelineService>,
    Path(room_id): Path<Uuid>,
    Query(params): Query<TimelineQueryParam>,
) -> Result<Json<TimelinePage>, AppError> {
    let page = timeline.scroll_chat_timeline(user.subject, room_id, params.timestamp).await?;
    Ok(Json(page))
}

pub async fn handle_get_users_in_room(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    Path(room_id): Path<Uuid>,
) -> Result<Json<Vec<RoomMember>>, AppError> {
    let users = rooms.get_users_in_room(user.subject, room_id).await?;
    Ok(Json(users))
}

pub async fn handle_get_joined_rooms(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    Query(params): Query<RoomListQueryParams>,
) -> Result<Json<CursorResults<ChatRoomDto>>, AppError> {
    let cursor: RoomPaginationCursor = decode_cursor(params.cursor).map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = clamp_page_size(params.limit);

    let rooms = rooms.get_joined_rooms(user.subject, params.name, cursor, page_size).await?;
    Ok(Json(rooms))
}

pub async fn handle_get_share_targets(
    State(share): State<ShareService>,
    user: CurrentUser,
    Query(params): Query<RoomListQueryParams>,
) -> Result<Json<CursorResults<ShareTarget>>, AppError> {
    let cursor: ShareTargetCursor = decode_cursor(params.cursor).map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = clamp_page_size(params.limit);

    let targets = share.get_share_targets(user.subject, params.name, cursor, page_size).await?;
    Ok(Json(targets))
}

pub async fn handle_get_room_with_details(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    Path(room_id): Path<Uuid>,
) -> Result<Json<ChatRoomWithUserDTO>, AppError> {
    let room = rooms.get_room_with_details(user.subject, room_id).await?;
    Ok(Json(room))
}

pub async fn mark_room_as_read(State(rooms): State<RoomService>, user: CurrentUser, Path(room_id): Path<Uuid>) -> Result<(), AppError> {
    rooms.mark_room_as_read(user.subject, room_id).await?;
    Ok(())
}

pub async fn handle_create_room(State(rooms): State<RoomService>, user: CurrentUser, Json(payload): Json<NewRoom>) -> Result<Json<ChatRoomDto>, AppError> {
    // Syntactic validation belongs here; every rule that needs a database read — block lists,
    // room cardinality, "this pair already has a room" — is enforced by the service.
    if let Some(first_message) = &payload.first_message {
        first_message.validate().map_err(AppError::from)?;
    }

    let room = rooms.create_room(user.subject, payload).await?;
    Ok(Json(room))
}

pub async fn handle_get_room_list_item_by_id(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path(room_id): Path<Uuid>,
) -> Result<Json<ChatRoomDto>, AppError> {
    let room = rooms.get_room_list_item_by_id(user.subject, room_id).await?;
    Ok(Json(room))
}

pub async fn handle_leave_room(user: CurrentUser, State(rooms): State<RoomService>, Path(room_id): Path<Uuid>) -> Result<(), AppError> {
    rooms.leave_room(user.subject, room_id).await?;
    Ok(())
}

pub async fn handle_invite_to_room(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path((room_id, invited_user_id)): Path<(Uuid, Uuid)>,
) -> Result<(), AppError> {
    rooms.invite_to_room(user.subject, room_id, invited_user_id).await?;
    Ok(())
}

pub async fn handle_search_existing_single_room(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Query(params): Query<RoomSearchQueryParam>,
) -> Result<Json<Option<Uuid>>, AppError> {
    let result = rooms.find_existing_single_room(&user.subject, &params.with_user).await?;
    Ok(Json(result))
}

pub async fn handle_save_room_image(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path(room_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    // Pulling the field out of the multipart body is parsing, which is the handler's job; the
    // service decides whether this caller may change the room's image.
    let mut image_data: Option<Bytes> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() == Some("image") {
                    let data = match field.bytes().await {
                        Ok(data) => data,
                        Err(_) => {
                            return Err(AppError::Validation("Error reading the image byte stream.".to_string()));
                        }
                    };
                    image_data = Some(data);
                    break;
                }
            }
            Ok(None) => {
                break; //stream finished
            }
            Err(err) => {
                //read error
                error!(error = %err, "Bad image upload");
                return Err(AppError::Validation("Error reading the image byte stream.".to_string()));
            }
        }
    }

    if let Some(image_data) = image_data {
        let response = rooms.set_room_image(user.subject, room_id, image_data).await?;
        Ok(Json(response))
    } else {
        Err(AppError::Validation("Required field 'image' not found in the upload.".to_string()))
    }
}

pub async fn handle_get_read_states(user: CurrentUser, State(rooms): State<RoomService>, Path(room_id): Path<Uuid>) -> Result<Json<Vec<RoomMember>>, AppError> {
    let read_states = rooms.get_read_states(user.subject, room_id).await?;
    Ok(Json(read_states))
}
