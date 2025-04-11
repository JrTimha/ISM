use crate::core::ISMConfig;
use crate::database::{MessageRepository, RoomDatabaseClient};

#[derive(Debug, Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub room_repository: RoomDatabaseClient,
    pub message_repository: MessageRepository
}