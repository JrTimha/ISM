use crate::utils::truncate_and_serialize;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;
use crate::model::room_member::RoomMember;


#[derive(sqlx::FromRow, sqlx::Type, Debug)]
pub struct ChatRoomEntity {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub room_image_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub latest_message: Option<DateTime<Utc>>,
    pub latest_message_preview_text: Option<String>,
    pub unread: Option<bool>
}

impl ChatRoomEntity {

    pub fn to_dto(&self) -> ChatRoomDto {

        let last_message = match self.latest_message_preview_text.as_ref() {
            Some(text) => serde_json::from_str::<LastMessagePreviewText>(text).unwrap_or(LastMessagePreviewText::New),
            None => LastMessagePreviewText::New
        };

        ChatRoomDto {
            id: self.id,
            room_type: self.room_type.clone(),
            room_image_url: self.room_image_url.clone(),
            room_name: self.room_name.clone(),
            created_at: self.created_at,
            latest_message: self.latest_message,
            unread: self.unread,
            latest_message_preview_text: last_message
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoomDto {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_image_url: Option<String>,
    pub room_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub latest_message: Option<DateTime<Utc>>,
    pub unread: Option<bool>,
    pub latest_message_preview_text: LastMessagePreviewText
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRoomWithUserDTO {
    #[serde(flatten)]
    pub room: ChatRoomDto,
    pub users: Vec<RoomMember>
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum LastMessagePreviewText {
    Text {
        sender_username: String,
        #[serde(serialize_with = "truncate_and_serialize")]
        text: String
    },
    Media {
        sender_username: String,
        media_type: String
    },
    Reply {
        sender_username: String,
        #[serde(serialize_with = "truncate_and_serialize")]
        reply_text: String
    },
    RoomChange {
        sender_username: String,
        room_change_type: RoomChangeType
    },
    New
}


#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NewRoom {
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub invited_users: Vec<Uuid>
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub enum RoomChangeType {
    LEAVE,
    JOIN,
    INVITE
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


