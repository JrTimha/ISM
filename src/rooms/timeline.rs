use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use log::{error};
use serde::Deserialize;
use uuid::Uuid;
use crate::errors::{ErrorCode, HttpError};
use crate::utils::{check_user_in_room};
use crate::core::AppState;
use crate::keycloak::decode::KeycloakToken;
use crate::messaging::model::MessageDTO;


#[derive(Deserialize)]
pub struct TimelineQuery {
    timestamp: DateTime<Utc>
}

pub async fn scroll_chat_timeline(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>,
    Query(params): Query<TimelineQuery>
) -> impl IntoResponse {
    
    if let Err(err) = check_user_in_room(&state, &token.subject, &room_id).await {
        return err.into_response();
    }
    match state.message_repository.fetch_data(params.timestamp, room_id).await {
        Ok(data) => {
            let mut mapped: Vec<MessageDTO> = vec![];
            data.into_iter().for_each(|message| {
               match message.to_dto() {
                   Ok(dto) => mapped.push(dto),
                   Err(err) => {
                       error!("Failed to convert message to DTO: {}", err);
                   }
               }
            });
            Json(mapped).into_response()
        },
        Err(err) => {
            error!("{}", err.to_string());
            HttpError::bad_request(ErrorCode::UnexpectedError, "Unable to fetch message data.").into_response()
        }
    }
}