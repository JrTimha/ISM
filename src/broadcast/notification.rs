use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::messaging::model::MessageDTO;
use crate::model::{ChatRoomDto, LastMessagePreviewText};
use crate::user_relationship::model::User;


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    #[serde(flatten)]
    pub body: NotificationEvent,
    pub created_at: DateTime<Utc>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum NotificationEvent {
    
    #[serde(rename_all = "camelCase")]
    FriendRequestReceived {from_user: User},

    #[serde(rename_all = "camelCase")]
    FriendRequestAccepted {from_user: User},

    /**
    * Different chat messages, sent to all active users in a room
    */
    #[serde(rename_all = "camelCase")]
    ChatMessage {message: MessageDTO, room_preview_text: LastMessagePreviewText },

    /**
    * A system message is a message not sent by a user, but by the system, whatever you want
    */
    SystemMessage {message: serde_json::Value},
    
    /**
    * Sending this event to a newly invited user
    */
    #[serde(rename_all = "camelCase")]
    NewRoom {room: ChatRoomDto, created_by: User },

    /**
    * Sending this event to a user who has left a room
    */
    #[serde(rename_all = "camelCase")]
    LeaveRoom {room_id: Uuid},

    /**
    * Sending this event to all users in a room where a member has left
    */
    #[serde(rename_all = "camelCase")]
    RoomChangeEvent {message: MessageDTO, room_preview_text: LastMessagePreviewText},

    /**
    * Sending this event to all users in a room when a user has read the latest message
    */
    #[serde(rename_all = "camelCase")]
    UserReadChat {user_id: Uuid, room_id: Uuid}
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SendNotification {
    #[serde(flatten)]
    pub body: Notification,
    pub to_user: Uuid
}