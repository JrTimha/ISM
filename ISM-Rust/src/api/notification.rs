use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

pub type NotificationCache = Arc<RwLock<HashMap<Uuid, Arc<RwLock<Vec<Notification>>>>>>;


#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Notification {
    pub notification_id: Uuid,
    pub notification_event: NotificationEvent,
    pub body: String, //json
    pub created_at: DateTime<Utc>
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum NotificationEvent {
    FriendRequest,
    ChatMessage,
    SystemMessage
}