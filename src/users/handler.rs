use crate::auth::CurrentUser;
use crate::core::AppState;
use crate::core::cursor::{CursorResults, clamp_page_size, decode_cursor};
use crate::core::errors::{AppError, AppResponse};
use crate::rooms::room_service::RoomService;
use crate::users::model::{
    RelationshipStateResponse, User, UserPaginationCursor, UserWithRelationshipDto,
};
use crate::users::query_param::{RelationshipQueryParams, UserSearchParams};
use crate::users::user_service::UserService;
use axum::Json;
use axum::extract::{Path, Query, State};
use std::sync::Arc;
use uuid::Uuid;

pub async fn handle_search_user_by_id(
    State(state): State<Arc<AppState>>,
    Path(target_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<Json<UserWithRelationshipDto>> {
    let user_dto = UserService::query_user_by_id(state, &user.subject, &target_id).await?;

    Ok(Json(user_dto))
}

pub async fn handle_search_user_by_name(
    State(state): State<Arc<AppState>>,
    user: CurrentUser,
    Query(params): Query<UserSearchParams>,
) -> AppResponse<Json<CursorResults<UserWithRelationshipDto>>> {
    let cursor: UserPaginationCursor = decode_cursor(params.cursor)
        .map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = clamp_page_size(params.limit);

    let search_results =
        UserService::query_user_by_name(state, &user.subject, &params.username, cursor, page_size)
            .await?;

    Ok(Json(search_results))
}

pub async fn handle_get_open_friend_requests(
    State(state): State<Arc<AppState>>,
    user: CurrentUser,
    Query(params): Query<RelationshipQueryParams>,
) -> AppResponse<Json<CursorResults<User>>> {
    let cursor: UserPaginationCursor = decode_cursor(params.cursor)
        .map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = clamp_page_size(params.limit);

    let results = UserService::get_open_friend_requests(
        state,
        &user.subject,
        params.username,
        cursor,
        page_size,
    )
    .await?;

    Ok(Json(results))
}

pub async fn handle_get_friends(
    State(state): State<Arc<AppState>>,
    user: CurrentUser,
    Query(params): Query<RelationshipQueryParams>,
) -> AppResponse<Json<CursorResults<User>>> {
    let cursor: UserPaginationCursor = decode_cursor(params.cursor)
        .map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = clamp_page_size(params.limit);

    let results =
        UserService::get_friends(state, &user.subject, params.username, cursor, page_size).await?;
    Ok(Json(results))
}

pub async fn handle_add_friend(
    State(state): State<Arc<AppState>>,
    Path(target_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<()> {
    if user.subject == target_id {
        return Err(AppError::Validation(
            "Cannot friendship yourself.".to_string(),
        ));
    }
    UserService::add_friend(state, user.subject, target_id).await?;
    Ok(())
}

pub async fn handle_accept_friend_request(
    State(state): State<Arc<AppState>>,
    Path(sender_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<()> {
    UserService::accept_friend_request(state, user.subject, sender_id).await?;
    Ok(())
}

pub async fn handle_reject_friend_request(
    State(state): State<Arc<AppState>>,
    Path(sender_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<()> {
    UserService::reject_friend_request(state, user.subject, sender_id).await?;
    Ok(())
}

pub async fn handle_remove_friend(
    State(state): State<Arc<AppState>>,
    Path(friend_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<()> {
    UserService::remove_friend(state, user.subject, friend_id).await?;
    Ok(())
}

pub async fn handle_ignore_user(
    State(state): State<Arc<AppState>>,
    Path(target_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<Json<RelationshipStateResponse>> {
    if user.subject == target_id {
        return Err(AppError::Validation("Cannot ignore yourself.".to_string()));
    }
    let updated_state = UserService::ignore_user(state.clone(), user.subject, target_id).await?;
    let room =
        RoomService::find_existing_single_room(state.clone(), &user.subject, &target_id).await?;
    if let Some(room) = room {
        RoomService::leave_room(state, user.subject, room).await?;
    }
    let response = RelationshipStateResponse {
        state: Some(updated_state),
    };
    Ok(Json(response))
}

pub async fn handle_undo_ignore_user(
    State(state): State<Arc<AppState>>,
    Path(target_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<Json<RelationshipStateResponse>> {
    let updated_state = UserService::undo_ignore(state, user.subject, target_id).await?;
    let response = RelationshipStateResponse {
        state: updated_state,
    };
    Ok(Json(response))
}
