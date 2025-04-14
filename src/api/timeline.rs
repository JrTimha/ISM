use std::str::FromStr;
use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use http::StatusCode;
use log::{error};
use serde::Deserialize;
use uuid::Uuid;
use crate::api::utils::{check_user_in_room, parse_uuid};
use crate::core::AppState;
use crate::keycloak::decode::KeycloakToken;
use crate::model::{Message, MessageDTO, MsgType};

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
    let id = parse_uuid(&token.subject).unwrap();
    if let Err(err) = check_user_in_room(&state, &id, &room_id).await {
        return err.into_response();
    }
    match state.message_repository.fetch_data(params.timestamp, room_id).await {
        Ok(data) => {
            let mut mapped: Vec<MessageDTO> = vec![];
            data.into_iter().for_each(|message| {
               match msg_to_dto(message) {
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
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub fn msg_to_dto(message: Message) -> Result<MessageDTO, Box<dyn std::error::Error>> {
    let msg = MessageDTO {
        chat_room_id: message.chat_room_id,
        message_id: message.message_id,
        sender_id: message.sender_id,
        msg_body: serde_json::from_str(&message.msg_body)?,
        msg_type: MsgType::from_str(&message.msg_type)?,
        created_at: message.created_at,
    };
    Ok(msg)
}