use async_trait::async_trait;
use log::info;
use redis::{AsyncTypedCommands, Client, ErrorKind, RedisError, RedisResult};
use redis::{aio::ConnectionManagerConfig};
use redis::aio::ConnectionManager;
use uuid::Uuid;
use crate::broadcast::Notification;
use crate::cache::cache_cleanup::periodic_cleanup_task;
use crate::cache::redis_subscriber::run_event_processor;
use crate::cache::util::{CHAT_CHANNEL, MASTER_INDEX_SET, NOTIFICATION, ROOM_CONTEXT, USER_NOTIFICATIONS, USER_SEQUENCE};
use crate::rooms::room_member::RoomContext;

/// TTL for the per-user sequence counter. Refreshed on every increment, so it only expires
/// after a user has been completely inactive for this long.
const SEQUENCE_TTL_SECONDS: i64 = 7 * 24 * 3600;

/// Outcome of a replay request. Either the missing notifications could be served from the
/// cache, or the client's last known sequence is too old (gap larger than the retention
/// window) and it must re-fetch authoritative state via REST.
#[derive(Debug)]
pub enum ReplayResult {
    Events(Vec<Notification>),
    ResyncNeeded,
}

#[async_trait]
pub trait Cache: Send + Sync {

    /// Allocate the next monotonic sequence number for a user. Returns `None` when sequencing
    /// is unavailable (no Redis), in which case durable events are delivered best-effort
    /// without replay support.
    async fn next_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>>;
    /// Return all durable notifications for a user with sequence strictly greater than
    /// `last_seq`, or `ResyncNeeded` if part of that range has already fallen out of the cache.
    async fn get_notifications_since_seq(&self, user_id: &Uuid, last_seq: u64) -> RedisResult<ReplayResult>;
    async fn add_notification_for_user(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<()>;
    async fn get_room_context(&self, room_id: &Uuid) -> RedisResult<Option<RoomContext>>;
    async fn set_room_context(&self, room_id: &Uuid, context: &RoomContext) -> RedisResult<()>;
    async fn invalidate_room_context(&self, room_id: &Uuid) -> RedisResult<()>;
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
        connection_manager.psubscribe(format!("{}*", CHAT_CHANNEL)).await?;

        info!("Established connection to the redis cache.");
        tokio::spawn(periodic_cleanup_task(connection_manager.clone()));
        tokio::spawn(run_event_processor(rx, connection_manager.clone()));
        Ok(Self { client: redis_client, connection: connection_manager })
    }
}


#[async_trait]
impl Cache for RedisCache {

    async fn next_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", USER_SEQUENCE, user_id);
        let seq = con.incr(&key, 1).await?;
        // Refresh the TTL so an active user's counter never disappears mid-session, while
        // counters for long-inactive users are eventually reclaimed.
        con.expire(&key, SEQUENCE_TTL_SECONDS).await?;
        Ok(Some(seq as u64))
    }

    async fn get_notifications_since_seq(&self, user_id: &Uuid, last_seq: u64) -> RedisResult<ReplayResult> {
        let mut con = self.connection.clone();
        let sorted_set_key = format!("{}{}", USER_NOTIFICATIONS, user_id);

        // Determine the oldest sequence still retained for this user. If the client's last
        // seen sequence is older than that, some events have already expired and we cannot
        // replay them losslessly -> the client must resync via REST.
        let oldest: Vec<(String, f64)> = redis::cmd("ZRANGE")
            .arg(&sorted_set_key)
            .arg(0)
            .arg(0)
            .arg("WITHSCORES")
            .query_async(&mut con)
            .await?;

        match oldest.first() {
            // Nothing retained for this user: nothing to replay.
            None => return Ok(ReplayResult::Events(vec![])),
            Some((_, oldest_score)) => {
                if (*oldest_score as u64) > last_seq + 1 {
                    return Ok(ReplayResult::ResyncNeeded);
                }
            }
        }

        // Fetch every notification key with sequence strictly greater than last_seq.
        let notification_keys: Vec<String> = con
            .zrangebyscore(
                &sorted_set_key,
                format!("({}", last_seq), // exclusive lower bound
                "+inf",
            )
            .await?;

        if notification_keys.is_empty() {
            return Ok(ReplayResult::Events(vec![]));
        }

        let notifications_json: Vec<Option<String>> = con.mget(&notification_keys).await?;

        // A missing value means the individual notification key expired while its index entry
        // still lingered -> the requested window has a hole, so the client must resync.
        if notifications_json.iter().any(|opt| opt.is_none()) {
            return Ok(ReplayResult::ResyncNeeded);
        }

        let notifications: Vec<Notification> = notifications_json
            .into_iter()
            .flatten()
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();

        Ok(ReplayResult::Events(notifications))
    }

    async fn add_notification_for_user(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<()> {
        let mut con = self.connection.clone();

        // Durable notifications must carry a sequence number; it is both their ordering score
        // and the cursor a reconnecting client replays from.
        let score = match notification.seq {
            Some(seq) => seq as f64,
            None => {
                return Err(RedisError::from((
                    ErrorKind::Client,
                    "Refusing to cache a notification without a sequence number",
                )));
            }
        };

        let notification_key = format!("{}{}", NOTIFICATION, Uuid::new_v4());
        let notification_json = serde_json::to_string(notification)
            .map_err(|err| {
                RedisError::from((
                    ErrorKind::Parse,
                    "Failed to serialize notification to JSON",
                    err.to_string(),
                ))
            })?;

        let sorted_set_key = format!("{}{}", USER_NOTIFICATIONS, user_id);

        let mut pipe = redis::pipe(); //like a atomic transaction
        pipe.atomic()
            //add k/v string
            .set_ex(
                &notification_key,
                notification_json,
                3600,  //ttl is 60 minutes
            )
            //add to sorted set from user
            .zadd(&sorted_set_key, &notification_key, score)
            //add to master index set, to track all user sets and remove them if they are empty
            .sadd(MASTER_INDEX_SET, user_id.to_string());

        pipe.exec_async(&mut con).await?;
        Ok(())
    }

    async fn get_room_context(&self, room_id: &Uuid) -> RedisResult<Option<RoomContext>> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", ROOM_CONTEXT, room_id);
        let json: Option<String> = con.get(&key).await?;
        Ok(json.and_then(|s| serde_json::from_str(&s).ok()))
    }

    async fn set_room_context(&self, room_id: &Uuid, context: &RoomContext) -> RedisResult<()> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", ROOM_CONTEXT, room_id);
        let json = serde_json::to_string(context).map_err(|err| {
            RedisError::from((ErrorKind::Parse, "Failed to serialize RoomContext", err.to_string()))
        })?;
        con.set_ex(&key, json, 900).await?;
        Ok(())
    }

    async fn invalidate_room_context(&self, room_id: &Uuid) -> RedisResult<()> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", ROOM_CONTEXT, room_id);
        con.del(&key).await?;
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


pub struct NoOpCache;

#[async_trait]
impl Cache for NoOpCache {

    async fn next_sequence(&self, _user_id: &Uuid) -> RedisResult<Option<u64>> {
        Ok(None)
    }
    async fn get_notifications_since_seq(&self, _user_id: &Uuid, _last_seq: u64) -> RedisResult<ReplayResult> {
        Ok(ReplayResult::Events(vec![]))
    }
    async fn add_notification_for_user(&self, _user_id: &Uuid, _notification: &Notification) -> RedisResult<()> {
        Ok(())
    }

    async fn get_room_context(&self, _room_id: &Uuid) -> RedisResult<Option<RoomContext>> {
        Ok(None)
    }

    async fn set_room_context(&self, _room_id: &Uuid, _context: &RoomContext) -> RedisResult<()> {
        Ok(())
    }

    async fn invalidate_room_context(&self, _room_id: &Uuid) -> RedisResult<()> {
        Ok(())
    }

    async fn publish_notification(&self, _notification: Notification, _channel_name: &String) -> RedisResult<()> {
        Ok(())
    }
}