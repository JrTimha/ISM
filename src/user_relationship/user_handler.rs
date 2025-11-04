use std::sync::Arc;
use axum::extract::{Path, State};
use axum::{Extension, Json};
use axum::response::IntoResponse;
use http::StatusCode;
use uuid::Uuid;
use crate::core::AppState;
use crate::errors::ErrorCode::UnexpectedError;
use crate::errors::{ErrorCode, HttpError};
use crate::keycloak::decode::KeycloakToken;
use crate::user_relationship::model::UserWithRelationshipDto;
use crate::user_relationship::utils::resolve_relationship_state;

pub async fn search_user_by_id(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> impl IntoResponse {
    match state.user_repository.find_user_by_id_with_relationship_type(&token.subject, &user_id).await {
        Ok(user) => {
            match user {
                None => HttpError::new(StatusCode::NOT_FOUND, ErrorCode::RoomNotFound, "Room not found").into_response(),
                Some(user) => {
                    let response = UserWithRelationshipDto {
                        user: user.r_user.clone(),
                        relationship_type: resolve_relationship_state(user.r_user.id, user.get_relationship())
                    };
                    Json(response).into_response()
                }
            }
        },
        Err(err) => HttpError::new(StatusCode::INTERNAL_SERVER_ERROR, UnexpectedError, err.to_string()).into_response()
    }
}
