use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use http::StatusCode;
use log::error;
use serde::Deserialize;
use uuid::Uuid;
use crate::api::utils::{check_user_in_room, parse_uuid};
use crate::core::AppState;
use crate::keycloak::decode::KeycloakToken;

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
            Json(data).into_response()
        },
        Err(err) => {
            error!("{}", err.to_string());
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}