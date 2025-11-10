use std::collections::HashSet;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::info;
use redis::{AsyncTypedCommands, Client, ErrorKind, RedisError, RedisResult};
use redis::{aio::ConnectionManagerConfig};
use redis::aio::ConnectionManager;
use uuid::Uuid;
use crate::broadcast::Notification;
use crate::cache::cache_cleanup::periodic_cleanup_task;
use crate::cache::redis_subscriber::run_event_processor;
use crate::cache::util::{CHAT_CHANNEL, MASTER_INDEX_SET, NOTIFICATION, ROOM_MEMBERS, USER_NOTIFICATIONS};

#[async_trait]
pub trait Cache: Send + Sync {

    async fn get_notifications_for_user(&self, user_id: &Uuid, latest_ts: DateTime<Utc>) -> RedisResult<Vec<Notification>>;
    async fn add_notification_for_user(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<()>;
    async fn add_user_to_room_cache(&self, user_id: &Uuid, room_id: &Uuid) -> RedisResult<()>;
    async fn remove_user_from_room_cache(&self, user_id: &Uuid, room_id: &Uuid) -> RedisResult<()>;
    async fn get_user_for_room(&self, room_id: &Uuid) -> RedisResult<Vec<Uuid>>;
    async fn set_user_for_room(&self, room_id: &Uuid, user_ids: &Vec<Uuid>) -> RedisResult<()>;
    async fn publish_notification(&self, notification: Notification, channel_name: &String) -> RedisResult<()>;

}

//docs: https://docs.rs/redis/latest/redis/
#[derive(Clone)]
#[allow(unused)]
pub struct RedisCache {
    client: Client,
    pub connection: ConnectionManager
}

impl RedisCache {
    pub async fn new(redis_url: String) -> RedisResult<Self> {
        let redis_client = Client::open(format!("{}/?protocol=3", redis_url))?;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let config = ConnectionManagerConfig::new()
            .set_push_sender(tx)
            .set_automatic_resubscription();

        let mut connection_manager = redis_client.get_connection_manager_with_config(config).await?;
        connection_manager.psubscribe(format!("{}*", CHAT_CHANNEL)).await?; //subscribe to all chat channels

        info!("Established connection to the redis cache.");
        tokio::spawn(periodic_cleanup_task(connection_manager.clone()));
        tokio::spawn(run_event_processor(rx, connection_manager.clone()));
        Ok(Self { client: redis_client, connection: connection_manager })
    }
}


#[async_trait]
impl Cache for RedisCache {


    async fn get_notifications_for_user(&self, user_id: &Uuid, latest_ts: DateTime<Utc>) -> RedisResult<Vec<Notification>> {
        let mut con = self.connection.clone();
        let sorted_set_key = format!("{}{}", USER_NOTIFICATIONS, user_id);
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
        let mut con = self.connection.clone();
        let notification_key = format!("{}{}", NOTIFICATION, Uuid::new_v4());
        let notification_json = serde_json::to_string(notification)
            .map_err(|err| {
                RedisError::from((
                    ErrorKind::Parse,
                    "Failed to serialize notification to JSON",
                    err.to_string(),
                ))
            })?;

        let score = notification.created_at.timestamp();
        let sorted_set_key = format!("{}{}", USER_NOTIFICATIONS, user_id);

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

    async fn add_user_to_room_cache(&self, user_id: &Uuid, room_id: &Uuid) -> RedisResult<()> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", ROOM_MEMBERS, room_id);
        let exists: bool = con.exists(&key).await?;

        if !exists { //if the member list is empty, we don't need to add the user to it
            return Ok(())
        }
        con.sadd(&key, user_id.to_string()).await?;
        Ok(())
    }

    async fn remove_user_from_room_cache(&self, user_id: &Uuid, room_id: &Uuid) -> RedisResult<()> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", ROOM_MEMBERS, room_id);
        con.srem(&key, user_id.to_string()).await?;
        Ok(())
    }

    async fn get_user_for_room(&self, room_id: &Uuid) -> RedisResult<Vec<Uuid>> {
        let mut conn = self.connection.clone();
        let key = format!("{}{}", ROOM_MEMBERS, room_id);

        let cached_user_ids: HashSet<String> = conn.smembers(&key).await?;
        if !cached_user_ids.is_empty() {
            let user_uuids = cached_user_ids
                .into_iter()
                .filter_map(|id_str| Uuid::parse_str(&id_str).ok())
                .collect();
            return Ok(user_uuids);
        }
        Ok(vec![])
    }


    async fn set_user_for_room(&self, room_id: &Uuid, user_ids: &Vec<Uuid>) -> RedisResult<()> {
        let mut conn = self.connection.clone();
        let key = format!("{}{}", ROOM_MEMBERS, room_id);

        if user_ids.is_empty() {
            conn.del(&key).await?;
            return Ok(());
        }

        let user_id_strs: Vec<String> = user_ids.iter().map(Uuid::to_string).collect();

        let mut pipe = redis::pipe();
        pipe.atomic()
            .del(&key)
            .sadd(&key, user_id_strs);

        pipe.exec_async(&mut conn).await?;
        Ok(())
    }

    async fn publish_notification(&self, notification: Notification, channel_name: &String) -> RedisResult<()> {
        let mut con = self.connection.clone();
        let notification_json = serde_json::to_string(&notification)
            .map_err(|err| {
                RedisError::from((
                    ErrorKind::Parse,
                    "Failed to serialize notification to JSON",
                    err.to_string(),
                ))
            })?;
        con.publish(channel_name, notification_json).await?;
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

    async fn add_user_to_room_cache(&self, _user_id: &Uuid, _room_id: &Uuid) -> RedisResult<()> {
        Ok(())
    }

    async fn remove_user_from_room_cache(&self, _user_id: &Uuid, _room_id: &Uuid) -> RedisResult<()> {
        Ok(())
    }

    async fn get_user_for_room(&self, _room_id: &Uuid) -> RedisResult<Vec<Uuid>> {
        Ok(vec![])
    }

    async fn set_user_for_room(&self, _room_id: &Uuid, _user_ids: &Vec<Uuid>) -> RedisResult<()> {
        Ok(())
    }

    async fn publish_notification(&self, _notification: Notification, _channel_name: &String) -> RedisResult<()> {
        Ok(())
    }
}

