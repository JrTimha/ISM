use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, sqlx::FromRow, sqlx::Type, Clone)]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
    pub profile_picture: Option<String>,
    pub room_id: Uuid,
    pub joined_at: DateTime<Utc>,
    pub last_message_read_at: Option<DateTime<Utc>>
}