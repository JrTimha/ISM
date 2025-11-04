use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::model::{ChatRoom, MessageDTO};
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
    ChatMessage {message: MessageDTO, display_value: String},

    /**
    * A system message is a message not sent by a user, but by the system, whatever you want
    */
    SystemMessage {message: serde_json::Value},
    
    /**
    * Sending this event to a newly invited user
    */
    NewRoom {room: ChatRoom },

    /**
    * Sending this event to a user who has left a room
    */
    #[serde(rename_all = "camelCase")]
    LeaveRoom {room_id: Uuid},

    /**
    * Sending this event to all users in a room where a member has left
    */
    RoomChangeEvent {message: MessageDTO}
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SendNotification {
    #[serde(flatten)]
    pub body: Notification,
    pub to_user: Uuid
}