use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;
use crate::database::User;

#[derive(Deserialize, Serialize, sqlx::FromRow, sqlx::Type, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoomEntity {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_name: String,
    pub created_at: DateTime<Utc>,
    pub latest_message: Option<DateTime<Utc>>
}

#[derive(sqlx::FromRow, Debug)]
pub struct ChatRoomParticipantEntity {
    pub user_id: Uuid,
    pub room_id: Uuid,
    pub joined_at: DateTime<Utc>,
    pub last_message_read_at: Option<DateTime<Utc>>
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NewRoom {
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub invited_users: Vec<Uuid>
}


#[derive(Debug, Deserialize, Serialize, Clone, Type)]
#[sqlx(type_name = "room_type")]
pub enum RoomType {
    Single,
    Group
}

impl RoomType {
    pub fn to_str(&self) -> &str {
        match self {
            RoomType::Single => "Single",
            RoomType::Group => "Group"
        }
    }

    pub fn to_string(&self) -> String {
        match self {
            RoomType::Single => String::from("Single"),
            RoomType::Group => String::from("Group")
        }
    }

}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoomDTO {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_name: String,
    pub created_at: DateTime<Utc>,
    pub users: Vec<User>
}

#[derive(Debug, Deserialize, Serialize, sqlx::FromRow, sqlx::Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoomListItemDTO {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_image_url: Option<String>,
    pub room_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub latest_message: Option<DateTime<Utc>>,
    pub unread: Option<bool>,
    pub latest_message_preview_text: Option<String>
}