//! In-memory [`Cache`] implementations for unit tests.
//!
//! `.claude/rules/architecture.md` rejects mocking *repositories*, and for a good reason: there is
//! no repository trait, so a mock would only prove that the mock behaves like the mock. [`Cache`]
//! is the opposite case — a genuinely runtime-polymorphic trait that already has two production
//! implementations ([`RedisCache`](super::redis_cache::RedisCache) and
//! [`NoOpCache`](super::redis_cache::NoOpCache)). A third, in-memory one is another real
//! implementation of a real contract, not a stand-in, which is why it is named `InMemoryCache`.
//!
//! It lives here rather than inside one module's test block so that the broadcast layer and
//! `NotificationService` can share it: replay and resync behaviour is exactly what needed covering,
//! and it needed covering in both.

use crate::broadcast::Notification;
use crate::cache::redis_cache::{Cache, ReplayResult};
use crate::rooms::model::RoomContext;
use async_trait::async_trait;
use redis::{ErrorKind, RedisError, RedisResult};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use uuid::Uuid;

/// One user's sequence counter and retained replay entries.
#[derive(Default)]
struct UserStream {
    /// Highest sequence ever issued — what `INCR user_seq:{id}` would return.
    counter: u64,
    /// Retained entries, oldest first. The notification is stored with `seq` stripped, mirroring
    /// Redis, where the sequence lives in the entry ID rather than in the payload.
    entries: VecDeque<(u64, Notification)>,
}

/// A `Cache` backed by a `HashMap`, reproducing the sequencing and both resync rules.
#[derive(Default)]
pub struct InMemoryCache {
    users: Mutex<HashMap<Uuid, UserStream>>,
}

impl InMemoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total retained entries across all users.
    pub fn cached_count(&self) -> usize {
        self.users.lock().expect("cache mutex").values().map(|stream| stream.entries.len()).sum()
    }

    /// Drop all but the newest `keep` entries for a user, without touching the counter — the
    /// in-memory equivalent of `XTRIM MAXLEN`, and the way to reach the "gap already trimmed away"
    /// branch of [`Cache::get_notifications_since_seq`] without a live Redis.
    pub fn trim_to(&self, user_id: &Uuid, keep: usize) {
        let mut users = self.users.lock().expect("cache mutex");
        if let Some(stream) = users.get_mut(user_id) {
            while stream.entries.len() > keep {
                stream.entries.pop_front();
            }
        }
    }
}

#[async_trait]
impl Cache for InMemoryCache {
    async fn append_notification(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<Option<u64>> {
        let mut users = self.users.lock().expect("cache mutex");
        let stream = users.entry(*user_id).or_default();

        stream.counter += 1;
        let seq = stream.counter;

        // Stored without `seq`, exactly as `RedisCache` stores it: the sequence is the entry key.
        // A fake that kept the sequence in the payload would hide the bug this pins.
        stream.entries.push_back((
            seq,
            Notification {
                seq: None,
                ..notification.clone()
            },
        ));

        Ok(Some(seq))
    }

    async fn current_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>> {
        let users = self.users.lock().expect("cache mutex");
        Ok(Some(users.get(user_id).map_or(0, |stream| stream.counter)))
    }

    async fn get_notifications_since_seq(&self, user_id: &Uuid, last_seq: u64) -> RedisResult<ReplayResult> {
        let users = self.users.lock().expect("cache mutex");
        let stream = match users.get(user_id) {
            Some(stream) => stream,
            None if last_seq > 0 => return Ok(ReplayResult::ResyncNeeded),
            None => return Ok(ReplayResult::Events(vec![])),
        };

        // The client's cursor is ahead of the counter: the sequence space was reset.
        if last_seq > stream.counter {
            return Ok(ReplayResult::ResyncNeeded);
        }

        // The gap starts before the oldest retained entry: it cannot be replayed losslessly.
        match stream.entries.front().map(|(seq, _)| *seq) {
            None => return Ok(ReplayResult::Events(vec![])),
            Some(oldest) if oldest > last_seq + 1 => return Ok(ReplayResult::ResyncNeeded),
            Some(_) => {}
        }

        let events = stream
            .entries
            .iter()
            .filter(|(seq, _)| *seq > last_seq)
            .map(|(seq, notification)| Notification {
                seq: Some(*seq),
                ..notification.clone()
            })
            .collect();

        Ok(ReplayResult::Events(events))
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

/// A `Cache` where every operation fails, for the error branches that a working cache cannot reach.
pub struct FailingCache;

impl FailingCache {
    fn error() -> RedisError {
        RedisError::from((ErrorKind::Client, "cache unavailable"))
    }
}

#[async_trait]
impl Cache for FailingCache {
    async fn append_notification(&self, _user_id: &Uuid, _notification: &Notification) -> RedisResult<Option<u64>> {
        Err(Self::error())
    }

    async fn current_sequence(&self, _user_id: &Uuid) -> RedisResult<Option<u64>> {
        Err(Self::error())
    }

    async fn get_notifications_since_seq(&self, _user_id: &Uuid, _last_seq: u64) -> RedisResult<ReplayResult> {
        Err(Self::error())
    }

    async fn get_room_context(&self, _room_id: &Uuid) -> RedisResult<Option<RoomContext>> {
        Err(Self::error())
    }

    async fn set_room_context(&self, _room_id: &Uuid, _context: &RoomContext) -> RedisResult<()> {
        Err(Self::error())
    }

    async fn invalidate_room_context(&self, _room_id: &Uuid) -> RedisResult<()> {
        Err(Self::error())
    }

    async fn publish_notification(&self, _notification: Notification, _channel_name: &String) -> RedisResult<()> {
        Err(Self::error())
    }
}
