use crate::auth::CurrentUser;
use crate::core::ValidatedQuery;
use crate::core::cursor::{CursorResults, decode_cursor};
use crate::core::errors::{AppError, AppResponse};
use crate::users::UserService;
use crate::users::model::UserPaginationCursor;
use crate::users::request::{FriendListQuery, UserSearchQuery};
use crate::users::response::{RelationshipStateResponse, UserProfileResponse, UserWithRelationshipResponse};
use axum::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

pub async fn handle_search_user_by_id(
    State(users): State<UserService>,
    Path(target_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<Json<UserWithRelationshipResponse>> {
    let response = users.query_user_by_id(&user.subject, &target_id).await?;

    Ok(Json(response))
}

pub async fn handle_search_user_by_name(
    State(users): State<UserService>,
    user: CurrentUser,
    ValidatedQuery(params): ValidatedQuery<UserSearchQuery>,
) -> AppResponse<Json<CursorResults<UserWithRelationshipResponse>>> {
    let cursor: UserPaginationCursor = decode_cursor(params.cursor).map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = params.limit.get();

    let search_results = users.query_user_by_name(&user.subject, &params.username, cursor, page_size).await?;

    Ok(Json(search_results))
}

pub async fn handle_get_open_friend_requests(
    State(users): State<UserService>,
    user: CurrentUser,
    ValidatedQuery(params): ValidatedQuery<FriendListQuery>,
) -> AppResponse<Json<CursorResults<UserProfileResponse>>> {
    let cursor: UserPaginationCursor = decode_cursor(params.cursor).map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = params.limit.get();

    let results = users.get_open_friend_requests(&user.subject, params.username, cursor, page_size).await?;

    Ok(Json(results))
}

pub async fn handle_get_friends(
    State(users): State<UserService>,
    user: CurrentUser,
    ValidatedQuery(params): ValidatedQuery<FriendListQuery>,
) -> AppResponse<Json<CursorResults<UserProfileResponse>>> {
    let cursor: UserPaginationCursor = decode_cursor(params.cursor).map_err(|_| AppError::Validation("Invalid Cursor-Parameters.".to_string()))?;
    let page_size = params.limit.get();

    let results = users.get_friends(&user.subject, params.username, cursor, page_size).await?;
    Ok(Json(results))
}

pub async fn handle_add_friend(State(users): State<UserService>, Path(target_id): Path<Uuid>, user: CurrentUser) -> AppResponse<()> {
    if user.subject == target_id {
        return Err(AppError::Validation("Cannot friendship yourself.".to_string()));
    }
    users.add_friend(user.subject, target_id).await?;
    Ok(())
}

pub async fn handle_accept_friend_request(State(users): State<UserService>, Path(sender_id): Path<Uuid>, user: CurrentUser) -> AppResponse<()> {
    users.accept_friend_request(user.subject, sender_id).await?;
    Ok(())
}

pub async fn handle_reject_friend_request(State(users): State<UserService>, Path(sender_id): Path<Uuid>, user: CurrentUser) -> AppResponse<()> {
    users.reject_friend_request(user.subject, sender_id).await?;
    Ok(())
}

pub async fn handle_remove_friend(State(users): State<UserService>, Path(friend_id): Path<Uuid>, user: CurrentUser) -> AppResponse<()> {
    users.remove_friend(user.subject, friend_id).await?;
    Ok(())
}

pub async fn handle_ignore_user(
    State(users): State<UserService>,
    Path(target_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<Json<RelationshipStateResponse>> {
    if user.subject == target_id {
        return Err(AppError::Validation("Cannot ignore yourself.".to_string()));
    }
    // Leaving the shared 1-1 room is part of blocking, not part of this endpoint — the service
    // owns the whole sequence now.
    let updated_state = users.ignore_user(user.subject, target_id).await?;
    let response = RelationshipStateResponse { state: Some(updated_state) };
    Ok(Json(response))
}

pub async fn handle_undo_ignore_user(
    State(users): State<UserService>,
    Path(target_id): Path<Uuid>,
    user: CurrentUser,
) -> AppResponse<Json<RelationshipStateResponse>> {
    let updated_state = users.undo_ignore(user.subject, target_id).await?;
    let response = RelationshipStateResponse { state: updated_state };
    Ok(Json(response))
}
