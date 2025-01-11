use chrono::{DateTime, Utc};
use scylla::DeserializeRow;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(DeserializeRow, Debug, Deserialize, Serialize, Clone)]
#[allow(unused)]
pub struct Message {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: String,
    pub msg_type: String,
    pub created_at: DateTime<Utc>
}

#[derive(Deserialize, Serialize, Debug)]
pub struct NewMessage {
    pub chat_room_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: String,
    pub msg_type: String
}

#[derive(DeserializeRow, Debug)]
pub struct ChatRoom {
    pub room_id: Uuid,
    pub room_type: String,
    pub room_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(DeserializeRow, Debug)]
pub struct ChatRoomParticipant {
    pub chat_room_id: Uuid,
    pub user_id: Uuid,
    pub joined_at: DateTime<Utc>
}

#[derive(DeserializeRow, Debug)]
pub struct Notification {
    pub notification_id: Uuid,
    pub user_id: Uuid,
    pub notification_type: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub is_read: bool
}