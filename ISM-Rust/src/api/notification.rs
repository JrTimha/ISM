use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time;
use uuid::Uuid;


#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub notification_event: NotificationEvent,
    pub body: serde_json::Value, //json
    pub created_at: DateTime<Utc>
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

type NotificationCache = Arc<RwLock<HashMap<Uuid, RwLock<Vec<Notification>>>>>;

#[derive(Clone)]
pub struct CacheService {
    cache: NotificationCache,
}

impl CacheService {

    pub fn new() -> Self {
        let notifications: NotificationCache = Arc::new(RwLock::new(HashMap::new()));
        CacheService { cache: notifications }
    }

    pub async fn add_notification(&self, user_id: Uuid, notification: Notification) {
        let cache = self.cache.read().await;
        if let Some(notifications) = cache.get(&user_id) { //first expect he is in the map
            let mut notifications = notifications.write().await;
            notifications.push(notification);
        } else { //if user doesn't exist, add him to the map
            drop(cache); //we need write access
            let mut cache = self.cache.write().await;
            let notifications = cache.entry(user_id).or_insert_with(|| RwLock::new(Vec::new()));
            let mut notifications = notifications.write().await;
            notifications.push(notification);
        }
    }

    pub async fn add_notifications_to_all(&self, user_ids: Vec<Uuid>, notification: Notification) {
        let mut cache = self.cache.write().await;
        for user_id in user_ids {
            if let Some(notifications) = cache.get_mut(&user_id) {
                let mut notifications = notifications.write().await;
                notifications.push(notification.clone());
            } else {
                let notifications = cache.entry(user_id).or_insert_with(|| RwLock::new(Vec::new()));
                let mut notifications = notifications.write().await;
                notifications.push(notification.clone());
            }
        }
    }

    pub async fn get_notifications(&self, user_id: Uuid) -> Option<Vec<Notification>> {
        let cache = self.cache.read().await;
        if let Some(notifications) = cache.get(&user_id) {
            let mut notifications = notifications.write().await;
            let messages = notifications.drain(..).collect();
            Some(messages)
        } else {
            None
        }
    }

    /**
    * Only start this cleanup coroutine ONCE if you want to clean up unused user notifications.
    */
    pub fn start_cleanup_task(&self, max_age_seconds: i64) {
        let cache = self.cache.clone();
        tokio::spawn(async move {
            loop {
                let now = Utc::now();
                info!("Cleaning up notify-cache.");
                let mut users_to_remove = Vec::new();
                {
                    let mut cache = cache.write().await;
                    for (user_id, notifications) in cache.iter_mut() {
                        let notifications = notifications.write().await;
                        debug!("User {} has {} notifications in notify-cache", user_id, notifications.len());
                        if notifications.is_empty() {
                            users_to_remove.push(*user_id);
                        }
                    }
                    for user_id in users_to_remove {
                        cache.remove(&user_id);
                        debug!("Removed user {} from notify-cache", user_id);
                    }

                    for (_user_id, notifications) in cache.iter_mut() {
                        let mut notifications = notifications.write().await;
                        debug!("Cleaning up for {} notifications", notifications.len());
                        notifications.retain(|notification| {
                            let age = now - notification.created_at;
                            age.num_seconds() <= max_age_seconds
                        });
                        debug!("{} notifications left after cleanup", notifications.len());
                    }
                }
                time::sleep(time::Duration::from_secs(60)).await;
            }
        });
    }


}