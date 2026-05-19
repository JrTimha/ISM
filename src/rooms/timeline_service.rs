use std::sync::Arc;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use crate::core::AppState;
use crate::core::errors::{AppError, AppResponse};
use crate::messaging::model::MessageDto;

pub struct TimelineService;

impl TimelineService {

    pub async fn scroll_chat_timeline(
        state: Arc<AppState>,
        room_id: Uuid,
        timestamp: DateTime<Utc>
    ) -> AppResponse<Vec<MessageDto>> {
        let data = state.chat_repository.fetch_messages(room_id, timestamp).await?;
        Ok(data.into_iter().map(MessageDto::from).collect())
    }

}