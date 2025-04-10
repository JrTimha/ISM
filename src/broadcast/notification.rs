use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;


#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub notification_event: NotificationEvent,
    pub body: serde_json::Value, //json
    pub created_at: DateTime<Utc>,
    pub display_value: Option<String>
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum NotificationEvent {
    FriendRequestReceived,
    FriendRequestAccepted,
    ChatMessage,
    SystemMessage,
    NewRoom
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NewNotification {
    pub event_type: NotificationEvent,
    pub to_user: Uuid,
    pub body: serde_json::Value,
    pub created_at: DateTime<Utc>,
}
