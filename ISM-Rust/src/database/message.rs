use std::fmt;
use chrono::{DateTime, Utc};
use scylla::{DeserializeRow};
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
    pub msg_type: MsgType
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum MsgType {
    Text,
    Image,
    Video,
    System,
    Reply,
    Reaction
}

impl fmt::Display for MsgType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MsgType::Text => write!(f, "Text"),
            MsgType::Image => write!(f, "Image"),
            MsgType::Video => write!(f, "Video"),
            MsgType::System => write!(f, "System"),
            MsgType::Reply => write!(f, "Reply"),
            MsgType::Reaction => write!(f, "Reaction")
        }
    }
}

