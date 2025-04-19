use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;
use crate::model::user::User;

#[derive(Deserialize, Serialize, sqlx::FromRow, sqlx::Type, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoomEntity {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub room_image_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub latest_message: Option<DateTime<Utc>>,
    pub latest_message_preview_text: Option<String>
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NewRoom {
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub invited_users: Vec<Uuid>
}


#[derive(Debug, Deserialize, Serialize, Clone, Type, PartialEq)]
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
pub struct ChatRoomWithUserDTO {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub room_image_url: Option<String>,
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