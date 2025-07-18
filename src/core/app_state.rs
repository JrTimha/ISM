use crate::core::ISMConfig;
use crate::database::{MessageDatabase, ObjectDatabase, RoomDatabase};

#[derive(Debug, Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub room_repository: RoomDatabase,
    pub message_repository: MessageDatabase,
    pub s3_bucket: ObjectDatabase
}