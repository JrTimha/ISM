use std::sync::Arc;
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use uuid::Uuid;
use crate::core::AppState;
use crate::core::cursor::{decode_cursor, CursorResults};
use crate::errors::{AppError, AppResponse};
use crate::keycloak::decode::KeycloakToken;
use crate::rooms::room_service::RoomService;
use crate::user_relationship::model::{RelationshipStateResponse, User, UserPaginationCursor, UserWithRelationshipDto};
use crate::user_relationship::query_param::UserSearchParams;
use crate::user_relationship::user_service::UserService;


pub async fn handle_search_user_by_id(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> Result<Json<UserWithRelationshipDto>, AppError> {

    let user_dto = UserService::query_user_by_id(
        state,
        &token.subject,
        &user_id
    ).await?;

    Ok(Json(user_dto))
}

pub async fn handle_search_user_by_name(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Query(params): Query<UserSearchParams>
) -> Result<Json<CursorResults<UserWithRelationshipDto>>, AppError> {

    let cursor: UserPaginationCursor = decode_cursor(params.cursor)
        .map_err(|_| AppError::ValidationError("Invalid Cursor-Parameters.".to_string()))?;

    let search_results = UserService::query_user_by_name(
        state,
        &token.subject,
        &params.username,
        cursor
    ).await?;

    Ok(Json(search_results))
}

pub async fn handle_get_open_friend_requests(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> Result<Json<Vec<User>>, AppError> {

    let results = UserService::get_open_friend_requests(
        state,
        &token.subject
    ).await?;

    Ok(Json(results))
}

pub async fn handle_get_friends(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> Result<Json<Vec<User>>, AppError> {

    let results = UserService::get_friends(state, &token.subject).await?;
    Ok(Json(results))
}

pub async fn handle_add_friend(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> Result<(), AppError> {

    if token.subject == user_id {
        return Err(AppError::ValidationError("Cannot friendship yourself.".to_string()));
    }
    UserService::add_friend(state, token.subject, user_id).await?;
    Ok(())
}


pub async fn handle_accept_friend_request(
    State(state): State<Arc<AppState>>,
    Path(sender_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> Result<(), AppError> {
    UserService::accept_friend_request(state, token.subject, sender_id).await?;
    Ok(())
}

pub async fn handle_reject_friend_request(
    State(state): State<Arc<AppState>>,
    Path(sender_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> Result<(), AppError> {
    UserService::reject_friend_request(state, token.subject, sender_id).await?;
    Ok(())
}

pub async fn handle_remove_friend(
    State(state): State<Arc<AppState>>,
    Path(friend_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> Result<(), AppError> {
    UserService::remove_friend(state, token.subject, friend_id).await?;
    Ok(())
}

pub async fn handle_ignore_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
)-> AppResponse<Json<RelationshipStateResponse>> {

    if token.subject == user_id {
       return Err(AppError::ValidationError("Cannot ignore yourself.".to_string()));
    }
    let updated_state = UserService::ignore_user(state.clone(), token.subject.clone(), user_id.clone()).await?;
    let room = RoomService::find_existing_single_room(state.clone(), &token.subject, &user_id).await?;
    if let Some(room) = room {
        RoomService::leave_room(state, token.subject, room).await?;
    }
    let response = RelationshipStateResponse {
        state: Some(updated_state)
    };
    Ok(Json(response))
}

pub async fn handle_undo_ignore_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
)-> AppResponse<Json<RelationshipStateResponse>> {
    let updated_state = UserService::undo_ignore(state, token.subject, user_id).await?;
    let response = RelationshipStateResponse {
        state: updated_state
    };
    Ok(Json(response))
}