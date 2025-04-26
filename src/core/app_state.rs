use crate::core::ISMConfig;
use crate::database::{MessageDatabase, RoomDatabase};

#[derive(Debug, Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub room_repository: RoomDatabase,
    pub message_repository: MessageDatabase
}