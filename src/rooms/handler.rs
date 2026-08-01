use crate::auth::CurrentUser;
use crate::core::cursor::{CursorResults, decode_cursor};
use crate::core::errors::{AppError, AppResponse};
use crate::core::{ValidatedJson, ValidatedQuery};
use crate::messaging::response::TimelinePageResponse;
use crate::rooms::model::{RoomPaginationCursor, ShareTargetCursor};
use crate::rooms::request::{NewRoomRequest, RoomListQuery, RoomSearchQuery, TimelineQuery};
use crate::rooms::response::{RoomDetailResponse, RoomImageUploadResponse, RoomMemberResponse, RoomResponse, ShareTargetResponse};
use crate::rooms::{RoomService, ShareService, TimelineService};
use axum::Json;
use axum::extract::{Multipart, Path, State};
use bytes::Bytes;
use tracing::error;
use uuid::Uuid;

pub async fn handle_scroll_chat_timeline(
    user: CurrentUser,
    State(timeline): State<TimelineService>,
    Path(room_id): Path<Uuid>,
    ValidatedQuery(params): ValidatedQuery<TimelineQuery>,
) -> AppResponse<Json<TimelinePageResponse>> {
    let page = timeline.scroll_chat_timeline(user.subject, room_id, params.timestamp).await?;
    Ok(Json(page))
}

pub async fn handle_get_users_in_room(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    Path(room_id): Path<Uuid>,
) -> AppResponse<Json<Vec<RoomMemberResponse>>> {
    let users = rooms.get_users_in_room(user.subject, room_id).await?;
    Ok(Json(users))
}

pub async fn handle_get_joined_rooms(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    ValidatedQuery(params): ValidatedQuery<RoomListQuery>,
) -> AppResponse<Json<CursorResults<RoomResponse>>> {
    let cursor: RoomPaginationCursor = decode_cursor(params.cursor).map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = params.limit.get();

    let rooms = rooms.get_joined_rooms(user.subject, params.name, cursor, page_size).await?;
    Ok(Json(rooms))
}

pub async fn handle_get_share_targets(
    State(share): State<ShareService>,
    user: CurrentUser,
    ValidatedQuery(params): ValidatedQuery<RoomListQuery>,
) -> AppResponse<Json<CursorResults<ShareTargetResponse>>> {
    let cursor: ShareTargetCursor = decode_cursor(params.cursor).map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = params.limit.get();

    let targets = share.get_share_targets(user.subject, params.name, cursor, page_size).await?;
    Ok(Json(targets))
}

pub async fn handle_get_room_with_details(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    Path(room_id): Path<Uuid>,
) -> AppResponse<Json<RoomDetailResponse>> {
    let room = rooms.get_room_with_details(user.subject, room_id).await?;
    Ok(Json(room))
}

pub async fn mark_room_as_read(State(rooms): State<RoomService>, user: CurrentUser, Path(room_id): Path<Uuid>) -> AppResponse<()> {
    rooms.mark_room_as_read(user.subject, room_id).await?;
    Ok(())
}

/// Syntactic validation runs in the extractor; every rule that needs a database read — block
/// lists, "this pair already has a room" — is enforced by the service.
pub async fn handle_create_room(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    ValidatedJson(payload): ValidatedJson<NewRoomRequest>,
) -> AppResponse<Json<RoomResponse>> {
    let room = rooms.create_room(user.subject, payload).await?;
    Ok(Json(room))
}

pub async fn handle_get_room_list_item_by_id(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path(room_id): Path<Uuid>,
) -> AppResponse<Json<RoomResponse>> {
    let room = rooms.get_room_list_item_by_id(user.subject, room_id).await?;
    Ok(Json(room))
}

pub async fn handle_leave_room(user: CurrentUser, State(rooms): State<RoomService>, Path(room_id): Path<Uuid>) -> AppResponse<()> {
    rooms.leave_room(user.subject, room_id).await?;
    Ok(())
}

pub async fn handle_invite_to_room(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path((room_id, invited_user_id)): Path<(Uuid, Uuid)>,
) -> AppResponse<()> {
    rooms.invite_to_room(user.subject, room_id, invited_user_id).await?;
    Ok(())
}

pub async fn handle_search_existing_single_room(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    ValidatedQuery(params): ValidatedQuery<RoomSearchQuery>,
) -> AppResponse<Json<Option<Uuid>>> {
    let result = rooms.find_existing_single_room(&user.subject, &params.with_user).await?;
    Ok(Json(result))
}

pub async fn handle_save_room_image(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path(room_id): Path<Uuid>,
    mut multipart: Multipart,
) -> AppResponse<Json<RoomImageUploadResponse>> {
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

pub async fn handle_get_read_states(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path(room_id): Path<Uuid>,
) -> AppResponse<Json<Vec<RoomMemberResponse>>> {
    let read_states = rooms.get_read_states(user.subject, room_id).await?;
    Ok(Json(read_states))
}
