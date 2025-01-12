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
    SystemMessage,
    NewRoom
}

pub async fn init_notify_cache() -> NotificationCache {
    let notifications: NotificationCache = Arc::new(RwLock::new(HashMap::new()));
    let cache_clone = Arc::clone(&notifications);
    tokio::spawn(async move {
        cleanup_old_notifications(cache_clone).await;
    });
    notifications
}

async fn cleanup_old_notifications(cache: NotificationCache) {
    loop {
        // 5 Minuten = 300 Sekunden
        let expiration_duration = chrono::Duration::seconds(10);
        let now = Utc::now();
        // Zugriff auf die gesamte HashMap
        let map = cache.read().await;
        for (user_id, notifications) in map.iter() {
            let mut user_notifications = notifications.write().await;
            // Entferne alte Notifications
            user_notifications.retain(|notification| {
                (now - notification.created_at) < expiration_duration
            });

            if user_notifications.is_empty() {
                println!("Notifications for user {user_id} have been cleared.");
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
    }
}