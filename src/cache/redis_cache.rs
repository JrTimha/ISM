use async_trait::async_trait;
use log::info;
use redis::{AsyncTypedCommands, Client, ErrorKind, RedisError, RedisResult};
use redis::{aio::ConnectionManagerConfig};
use redis::aio::ConnectionManager;
use uuid::Uuid;
use crate::broadcast::Notification;
use crate::cache::redis_subscriber::run_event_processor;
use crate::cache::util::{CHAT_CHANNEL, ROOM_CONTEXT, USER_NOTIFICATIONS, USER_SEQUENCE};
use crate::rooms::room_member::RoomContext;

/// TTL for the per-user sequence counter and notification stream. Refreshed on every write, so a
/// key only expires after a user has been completely inactive for this long. This is what reclaims
/// storage for inactive users — there is no background cleanup task.
const SEQUENCE_TTL_SECONDS: i64 = 24 * 3600;

/// Approximate cap on retained notifications per user. `XADD ... MAXLEN ~ N` trims older entries on
/// every write (amortized O(1)), so the replay buffer is count-bounded instead of time-bounded.
/// A reconnecting client whose gap predates the retained window receives `ResyncNeeded`.
const STREAM_MAX_LEN: usize = 300;

/// Single field under which the serialized notification JSON is stored in each stream entry.
const STREAM_FIELD: &str = "data";

/// Decoded `XRANGE` reply: a list of `(entry_id, [(field, value), ...])`.
type StreamEntries = Vec<(String, Vec<(String, String)>)>;

/// Extract the numeric sequence from a `<seq>-<n>` stream entry ID.
fn parse_stream_seq(id: &str) -> Option<u64> {
    id.split('-').next()?.parse().ok()
}

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
    /// Read the highest sequence number currently issued to a user **without** advancing it.
    /// Returns `Some(0)` when no event has been issued yet, or `None` when sequencing is
    /// unavailable (no Redis). A freshly REST-synced client uses this as its replay baseline.
    async fn current_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>>;
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

    async fn current_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", USER_SEQUENCE, user_id);
        let current = con
            .get(&key)
            .await?
            .and_then(|raw: String| raw.parse().ok())
            .unwrap_or(0);
        Ok(Some(current))
    }

    async fn get_notifications_since_seq(&self, user_id: &Uuid, last_seq: u64) -> RedisResult<ReplayResult> {
        let mut con = self.connection.clone();
        let stream_key = format!("{}{}", USER_NOTIFICATIONS, user_id);
        let seq_key = format!("{}{}", USER_SEQUENCE, user_id);

        // The sequence counter is the highest seq ever issued to this user. If the client's cursor
        // is ahead of it, the server's sequence space has been reset (counter expired by TTL, or
        // the cache was flushed) and the client references sequences that no longer exist. Silently
        // continuing would let the dedup high-water swallow every new (now lower-numbered) event,
        // so we force a resync instead.
        let current_seq: u64 = con
            .get(&seq_key)
            .await?
            .and_then(|raw: String| raw.parse().ok())
            .unwrap_or(0);
        if last_seq > current_seq {
            return Ok(ReplayResult::ResyncNeeded);
        }

        // Determine the oldest sequence still retained for this user. If the client's last seen
        // sequence is older than that, the gap has already been trimmed out of the stream and we
        // cannot replay it losslessly -> the client must resync via REST. Because a stream is a
        // single structure, there is no separate index that can dangle: an entry is either present
        // or trimmed, so this is the only resync trigger.
        let oldest: StreamEntries = redis::cmd("XRANGE")
            .arg(&stream_key)
            .arg("-")
            .arg("+")
            .arg("COUNT")
            .arg(1)
            .query_async(&mut con)
            .await?;

        match oldest.first().and_then(|(id, _)| parse_stream_seq(id)) {
            // Nothing retained for this user: nothing to replay.
            None => return Ok(ReplayResult::Events(vec![])),
            Some(oldest_seq) => {
                if oldest_seq > last_seq + 1 {
                    return Ok(ReplayResult::ResyncNeeded);
                }
            }
        }

        // Fetch every entry with sequence strictly greater than last_seq. Entry IDs are `<seq>-0`,
        // so an exclusive lower bound of `(<last_seq>-0` yields exactly seq > last_seq, in order.
        let entries: StreamEntries = redis::cmd("XRANGE")
            .arg(&stream_key)
            .arg(format!("({}-0", last_seq))
            .arg("+")
            .query_async(&mut con)
            .await?;

        let notifications: Vec<Notification> = entries
            .into_iter()
            .filter_map(|(_, fields)| {
                fields
                    .into_iter()
                    .find(|(field, _)| field == STREAM_FIELD)
                    .and_then(|(_, json)| serde_json::from_str(&json).ok())
            })
            .collect();

        Ok(ReplayResult::Events(notifications))
    }

    async fn add_notification_for_user(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<()> {
        let mut con = self.connection.clone();

        // Durable notifications must carry a sequence number; it becomes the stream entry ID
        // (`<seq>-0`) and the cursor a reconnecting client replays from.
        let seq = match notification.seq {
            Some(seq) => seq,
            None => {
                return Err(RedisError::from((
                    ErrorKind::Client,
                    "Refusing to cache a notification without a sequence number",
                )));
            }
        };

        let notification_json = serde_json::to_string(notification)
            .map_err(|err| {
                RedisError::from((
                    ErrorKind::Parse,
                    "Failed to serialize notification to JSON",
                    err.to_string(),
                ))
            })?;

        let stream_key = format!("{}{}", USER_NOTIFICATIONS, user_id);

        let mut pipe = redis::pipe(); //single round trip: append (with trim) + refresh inactivity TTL
        pipe.atomic()
            // Append using the per-user seq as the explicit entry ID and trim to ~STREAM_MAX_LEN.
            // `~` lets Redis trim at node boundaries (amortized O(1)); it keeps at least N entries.
            .cmd("XADD")
                .arg(&stream_key)
                .arg("MAXLEN").arg("~").arg(STREAM_MAX_LEN)
                .arg(format!("{}-0", seq))
                .arg(STREAM_FIELD).arg(&notification_json)
                .ignore()
            // Refresh the TTL so an active user's stream never disappears mid-session, while a
            // fully inactive user's stream is eventually reclaimed without any cleanup task.
            .expire(&stream_key, SEQUENCE_TTL_SECONDS)
                .ignore();

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
    async fn current_sequence(&self, _user_id: &Uuid) -> RedisResult<Option<u64>> {
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