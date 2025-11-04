use std::sync::Arc;
use chrono::{DateTime, Utc};
use log::error;
use uuid::Uuid;
use crate::core::AppState;
use crate::errors::AppError;
use crate::messaging::model::MessageDTO;

pub struct TimelineService;

impl TimelineService {

    pub async fn scroll_chat_timeline(
        state: Arc<AppState>,
        room_id: Uuid,
        timestamp: DateTime<Utc>
    ) -> Result<Vec<MessageDTO>, AppError> {
        
        let data = state.message_repository.fetch_data(timestamp, room_id).await
            .map_err(|err| AppError::DatabaseError(err))?;
        
        let mut mapped: Vec<MessageDTO> = vec![];
        data.into_iter().for_each(|message| {
            match message.to_dto() {
                Ok(dto) => mapped.push(dto),
                Err(err) => {
                    error!("Failed to convert message to DTO: {}", err);
                }
            }
        });
        Ok(mapped)
    }
}