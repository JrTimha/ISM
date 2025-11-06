use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Client, ErrorKind, RedisError, RedisResult};
use uuid::Uuid;
use crate::broadcast::Notification;

const MASTER_INDEX_SET: &str = "active_user_notification_indices";

#[async_trait]
pub trait Cache: Send + Sync {
    async fn get_notifications_for_user(&self, user_id: &Uuid, latest_ts: DateTime<Utc>) -> RedisResult<Vec<Notification>>;
    async fn add_notification_for_user(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<()>;
}

//docs: https://docs.rs/redis/latest/redis/
#[derive(Clone)]
pub struct RedisCache {
    pub client: Client,
}


#[async_trait]
impl Cache for RedisCache {

    async fn get_notifications_for_user(&self, user_id: &Uuid, latest_ts: DateTime<Utc>) -> RedisResult<Vec<Notification>> {
        let mut con = self.client.get_multiplexed_async_connection().await?;
        let sorted_set_key = format!("user_notifications:{}", user_id);
        let min_score = latest_ts.timestamp();

        let notification_keys: Vec<String> = con
            .zrangebyscore(
                &sorted_set_key,
                min_score,      // timestamp of oldest notification
                "+inf",         // get all notifications
            )
            .await?;

        if notification_keys.is_empty() {
            return Ok(vec![]);
        }
        let notifications_json: Vec<Option<String>> = con.mget(&notification_keys).await?;
        let notifications: Vec<Notification> = notifications_json
            .into_iter()
            .filter_map(|opt_json| opt_json)
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();

        Ok(notifications)
    }

    async fn add_notification_for_user(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<()> {

        let mut con = self.client.get_multiplexed_async_connection().await?;
        let notification_key = format!("notification:{}", Uuid::new_v4());
        let notification_json = serde_json::to_string(notification)
            .map_err(|err| {
                RedisError::from((
                    ErrorKind::Parse,
                    "Failed to serialize notification to JSON",
                    err.to_string(),
                ))
            })?;

        let score = notification.created_at.timestamp();
        let sorted_set_key = format!("user_notifications:{}", user_id);

        let mut pipe = redis::pipe(); //like a atomic transaction
        pipe.atomic()
            //add k/v string
            .set_ex(
                &notification_key,
                notification_json,
                3600, //ttl is 60 minutes
            )
            //add to sorted set from user
            .zadd(&sorted_set_key, &notification_key, score)
            //add to master index set, to track all user sets and remove them if they are empty
            .sadd(MASTER_INDEX_SET, user_id.to_string());

        pipe.exec_async(&mut con).await?;
        Ok(())
    }

}


//doing nothing, used when redis is not available:
pub struct NoOpCache;

#[async_trait]
impl Cache for NoOpCache {
    async fn get_notifications_for_user(&self, _user_id: &Uuid, _latest_ts: DateTime<Utc>) -> RedisResult<Vec<Notification>> {
        Ok(vec![])
    }
    async fn add_notification_for_user(&self, _user_id: &Uuid, _notification: &Notification) -> RedisResult<()> {
        Ok(())
    }
}

