use crate::broadcast::Notification;
use crate::cache::util::{ROOM_CONTEXT, USER_NOTIFICATIONS, USER_SEQUENCE};
use crate::rooms::model::RoomContext;
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::{AsyncTypedCommands, Client, ErrorKind, RedisError, RedisResult, Script};
use std::sync::LazyLock;
use tracing::{info, warn};
use uuid::Uuid;

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

/// Compiled once. [`Script`] caches the SHA1 and `invoke_async` sends `EVALSHA`, falling back to
/// `SCRIPT LOAD` + retry on `NOSCRIPT`, so a Redis restart recovers on its own.
static APPEND_NOTIFICATION: LazyLock<Script> = LazyLock::new(|| Script::new(APPEND_NOTIFICATION_LUA));

/// Runs inside Redis so the sequence allocation and the stream append cannot come apart.
///
/// `MULTI`/`EXEC` cannot express this: the entry ID must be the value `INCR` returned, and a queued
/// transaction has no results until it executes. Splitting it across two round trips is what let a
/// sequence be allocated and then not stored — a hole mid-stream that no reader could detect,
/// because the gap check below only inspects the *oldest* retained entry.
///
/// Not Redis Cluster safe: the two keys share no hash tag, so they may live in different slots and
/// the script would be rejected with `CROSSSLOT`. Moving to a cluster means changing the key format
/// to `user_seq:{<uuid>}` / `user_notifications:{<uuid>}`, which invalidates every existing key.
const APPEND_NOTIFICATION_LUA: &str = r#"
-- Atomically allocate the next per-user sequence and append the event to that user's stream.
--
-- KEYS[1] sequence counter (user_seq:<uuid>)   KEYS[2] stream (user_notifications:<uuid>)
-- ARGV[1] TTL seconds   ARGV[2] approximate max stream length
-- ARGV[3] stream field name   ARGV[4] serialized notification, without `seq`
-- Returns the assigned sequence number.
--
-- A script is atomic in that nothing interleaves with it, NOT in that a failed command is rolled
-- back. So XADD must not be able to fail after INCR has run -- and it can: XADD rejects any
-- explicit ID that is not strictly greater than the stream's last-generated-id, and that survives
-- trimming. If the counter is lost while the stream survives (eviction under maxmemory is the
-- realistic trigger: the counter is tiny, the stream is not), INCR restarts at 1 and every write
-- for that user fails until the stream expires. The realign below closes that.

local function stream_last_seq(key)
  local info = redis.pcall('XINFO', 'STREAM', key)   -- pcall: XINFO errors on a missing key
  if type(info) ~= 'table' or info.err then return nil end
  for i = 1, #info, 2 do
    if info[i] == 'last-generated-id' then
      return tonumber(string.match(info[i + 1], '^(%d+)'))
    end
  end
  return nil
end

local seq = redis.call('INCR', KEYS[1])

if seq == 1 then
  -- The counter did not exist: either this is the user's first event, or the counter was reclaimed
  -- while the stream survived. Only in the second case is `1-0` too small, so this costs one extra
  -- command on a user's first write and nothing on the hot path.
  local last = stream_last_seq(KEYS[2])
  if last and last >= seq then
    seq = last + 1
    redis.call('SET', KEYS[1], string.format('%d', seq))
  end
end

-- `~` lets Redis trim at node boundaries (amortized O(1)); it keeps at least ARGV[2] entries.
redis.call('XADD', KEYS[2], 'MAXLEN', '~', ARGV[2], string.format('%d-0', seq), ARGV[3], ARGV[4])

-- Refreshed on every write, so the pair is reclaimed only after the user has been completely
-- inactive for the TTL -- there is no cleanup task. Both are set in the same atomic call now, so a
-- partial failure can no longer leave the counter outliving its stream.
redis.call('EXPIRE', KEYS[1], ARGV[1])
redis.call('EXPIRE', KEYS[2], ARGV[1])

return seq
"#;

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
    /// Allocate this user's next monotonic sequence number **and** append the event to their
    /// replay stream, in one atomic Redis call.
    ///
    /// Returns the assigned sequence, or `None` when sequencing is unavailable (no Redis), in
    /// which case the event is delivered best-effort without replay support.
    ///
    /// The stored JSON deliberately omits `seq`: the stream entry ID **is** `<seq>-0`, so the
    /// sequence has exactly one source and cannot disagree with itself.
    /// [`Self::get_notifications_since_seq`] re-attaches it on read.
    async fn append_notification(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<Option<u64>>;
    /// Read the highest sequence number currently issued to a user **without** advancing it.
    /// Returns `Some(0)` when no event has been issued yet, or `None` when sequencing is
    /// unavailable (no Redis). A freshly REST-synced client uses this as its replay baseline.
    async fn current_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>>;
    /// Return all durable notifications for a user with sequence strictly greater than
    /// `last_seq`, or `ResyncNeeded` if part of that range has already fallen out of the cache.
    async fn get_notifications_since_seq(&self, user_id: &Uuid, last_seq: u64) -> RedisResult<ReplayResult>;
    async fn get_room_context(&self, room_id: &Uuid) -> RedisResult<Option<RoomContext>>;
    async fn set_room_context(&self, room_id: &Uuid, context: &RoomContext) -> RedisResult<()>;
    async fn invalidate_room_context(&self, room_id: &Uuid) -> RedisResult<()>;
}

//docs: https://docs.rs/redis/latest/redis/
///
/// # Sharing
///
/// `RedisCache` is [`Clone`], and the [`ConnectionManager`] inside it is designed to be cloned:
/// it multiplexes every clone over one connection and reconnects transparently. Cloning it is the
/// intended way to share Redis access — every method here does `self.connection.clone()`.
///
/// Never reach for `Arc<Mutex<Connection>>`: that would funnel all Redis traffic through a single
/// lock and serialize requests that the manager is built to pipeline.
#[derive(Clone)]
pub struct RedisCache {
    pub connection: ConnectionManager,
}

impl RedisCache {
    /// Connects to Redis.
    ///
    /// Does **not** spawn anything: the cache is a request/response store, and every read and write
    /// happens on the caller's task.
    pub async fn connect(redis_url: String) -> RedisResult<Self> {
        let redis_client = Client::open(format!("{}/?protocol=3", redis_url))?;
        let connection = redis_client.get_connection_manager().await?;

        info!("Established connection to the Redis, caching enabled.");
        Ok(Self { connection })
    }
}

#[async_trait]
impl Cache for RedisCache {

    async fn append_notification(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<Option<u64>> {
        let mut con = self.connection.clone();

        // `seq` has exactly one source: the stream entry ID assigned below. Stripping it here
        // rather than at the call site makes that an invariant of the cache instead of a
        // convention every caller has to remember — an envelope that already carries a sequence
        // cannot store a second, disagreeing copy of it.
        let payload = match notification.seq {
            None => serde_json::to_string(notification),
            Some(_) => serde_json::to_string(&Notification {
                seq: None,
                ..notification.clone()
            }),
        }
        .map_err(|err| RedisError::from((ErrorKind::Parse, "Failed to serialize notification to JSON", err.to_string())))?;

        let seq: u64 = APPEND_NOTIFICATION
            .key(format!("{}{}", USER_SEQUENCE, user_id))
            .key(format!("{}{}", USER_NOTIFICATIONS, user_id))
            .arg(SEQUENCE_TTL_SECONDS)
            .arg(STREAM_MAX_LEN)
            .arg(STREAM_FIELD)
            .arg(payload)
            .invoke_async(&mut con)
            .await?;

        Ok(Some(seq))
    }

    async fn current_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", USER_SEQUENCE, user_id);
        let current = con.get(&key).await?.and_then(|raw: String| raw.parse().ok()).unwrap_or(0);
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
        let current_seq: u64 = con.get(&seq_key).await?.and_then(|raw: String| raw.parse().ok()).unwrap_or(0);
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

        // An entry we cannot decode is a lost event, not a skippable one: the caller derives its
        // high-water mark from what it received, so dropping the entry would advance the client's
        // cursor past an event it never got, with no way for either side to notice. This goes live
        // the moment the envelope format changes while a user's stream still holds older entries.
        let mut lost_entry = false;

        let notifications: Vec<Notification> = entries
            .into_iter()
            .filter_map(|(id, fields)| {
                let (_, json) = fields.into_iter().find(|(field, _)| field == STREAM_FIELD)?;
                match serde_json::from_str::<Notification>(&json) {
                    Ok(mut notification) => {
                        // The stored JSON carries no `seq`; the entry ID is where it lives. Entries
                        // written before that change do carry one, and it is the same number, so
                        // both formats replay identically.
                        notification.seq = parse_stream_seq(&id);
                        Some(notification)
                    }
                    Err(error) => {
                        warn!(%user_id, entry = %id, error = %error, "Unparsable cached notification, forcing resync");
                        lost_entry = true;
                        None
                    }
                }
            })
            .collect();

        if lost_entry {
            return Ok(ReplayResult::ResyncNeeded);
        }

        Ok(ReplayResult::Events(notifications))
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
        let json = serde_json::to_string(context).map_err(|err| RedisError::from((ErrorKind::Parse, "Failed to serialize RoomContext", err.to_string())))?;
        con.set_ex(&key, json, 900).await?;
        Ok(())
    }

    async fn invalidate_room_context(&self, room_id: &Uuid) -> RedisResult<()> {
        let mut con = self.connection.clone();
        let key = format!("{}{}", ROOM_CONTEXT, room_id);
        con.del(&key).await?;
        Ok(())
    }
}

pub struct NoOpCache;

#[async_trait]
impl Cache for NoOpCache {
    async fn append_notification(&self, _user_id: &Uuid, _notification: &Notification) -> RedisResult<Option<u64>> {
        Ok(None)
    }
    async fn current_sequence(&self, _user_id: &Uuid) -> RedisResult<Option<u64>> {
        Ok(None)
    }
    async fn get_notifications_since_seq(&self, _user_id: &Uuid, _last_seq: u64) -> RedisResult<ReplayResult> {
        Ok(ReplayResult::Events(vec![]))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcast::NotificationEvent;

    fn resync() -> Notification {
        Notification::new(NotificationEvent::Resync { reason: "too old".into() })
    }

    #[test]
    fn stream_ids_decode_to_their_sequence() {
        assert_eq!(parse_stream_seq("42-0"), Some(42));
        // Only the first component is the sequence; the rest is Redis' intra-millisecond counter.
        assert_eq!(parse_stream_seq("42-7"), Some(42));
        assert_eq!(parse_stream_seq("0-1"), Some(0));
        assert_eq!(parse_stream_seq("abc-0"), None);
        assert_eq!(parse_stream_seq("-0"), None);
        assert_eq!(parse_stream_seq(""), None);
    }

    /// The write path stores the envelope without `seq` and the read path restores it from the
    /// entry ID. This pins the round trip, which is what makes the entry ID the single source of
    /// the sequence.
    #[test]
    fn seq_survives_the_round_trip_through_the_entry_id() {
        let stored = Notification { seq: None, ..resync() };

        let json = serde_json::to_string(&stored).expect("serialize");
        assert!(!json.contains("\"seq\""), "the stored payload must not carry a sequence: {json}");

        let mut restored: Notification = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.seq, None);

        restored.seq = parse_stream_seq("42-0");
        assert_eq!(restored.seq, Some(42));

        // Everything else came back unchanged.
        let expected = Notification { seq: Some(42), ..stored };
        assert_eq!(
            serde_json::to_string(&restored).expect("re-serialize"),
            serde_json::to_string(&expected).expect("expected")
        );
    }

    /// Entries written before the sequence moved into the entry ID still carry it in the payload.
    /// Both formats must replay identically, which they do because the read path overwrites it
    /// with the same number.
    #[test]
    fn a_legacy_entry_with_an_inline_seq_replays_the_same() {
        let legacy = Notification { seq: Some(42), ..resync() };
        let json = serde_json::to_string(&legacy).expect("serialize");
        assert!(json.contains("\"seq\""));

        let mut restored: Notification = serde_json::from_str(&json).expect("deserialize");
        restored.seq = parse_stream_seq("42-0");

        assert_eq!(restored.seq, Some(42));
    }

    #[tokio::test]
    async fn the_noop_cache_reports_that_sequencing_is_unavailable() {
        let user_id = Uuid::new_v4();

        assert_eq!(NoOpCache.append_notification(&user_id, &resync()).await.expect("append"), None);
        assert_eq!(NoOpCache.current_sequence(&user_id).await.expect("current"), None);
        assert!(matches!(
            NoOpCache.get_notifications_since_seq(&user_id, 0).await.expect("replay"),
            ReplayResult::Events(events) if events.is_empty()
        ));
    }
}
