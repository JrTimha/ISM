use chrono::{DateTime, Utc};
use scylla::DeserializeRow;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(DeserializeRow, Debug, Deserialize, Serialize, Clone)]
#[allow(unused)]
pub struct Message {
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub receiver_id: Uuid,
    pub msg_body: String,
    pub msg_type: String,
    pub has_read: bool,
    pub created_at: DateTime<Utc>
}

#[derive(Deserialize, Serialize, Debug)]
pub struct NewMessage {
    pub receiver_id: Uuid,
    pub msg_body: String,
    pub msg_type: String
}