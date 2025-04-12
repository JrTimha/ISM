use std::sync::Arc;
use uuid::Uuid;
use crate::api::errors::HttpError;
use crate::core::AppState;
use crate::database::RoomRepository;

pub fn parse_uuid(subject: &str) -> Result<Uuid, HttpError> {
    Uuid::try_parse(subject).map_err(|_| HttpError::bad_request("Invalid token subject".to_string()))
}

pub async fn check_user_in_room(
    state: &Arc<AppState>,
    user_id: &Uuid,
    room_id: &Uuid,
) -> Result<(), HttpError> {
    let is_in = state
        .room_repository
        .is_user_in_room(user_id, room_id)
        .await
        .map_err(|_| HttpError::bad_request("Failed to check room access."))?;

    if is_in {
        Ok(())
    } else {
        Err(HttpError::unauthorized("Room not found or access denied."))
    }
}