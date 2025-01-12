use std::fmt;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::database::User;

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoom {
    pub room_id: Uuid,
    pub room_type: RoomType,
    pub room_name: String,
    pub created_at: DateTime<Utc>,
    pub users: Vec<ChatRoomParticipant>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoomParticipant {
    pub user: User,
    pub joined_at: DateTime<Utc>
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NewRoom {
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub invited_users: Vec<Uuid>
}


#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum RoomType {
    Single,
    Group
}

impl fmt::Display for RoomType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            RoomType::Single => write!(f, "Single"),
            RoomType::Group => write!(f, "Group")

        }
    }
}