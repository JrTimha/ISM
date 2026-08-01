//! Room-scoped broadcasting.
//!
//! Every room event follows the same two steps: resolve who is currently in the room, then fan the
//! event out to them. That pair was written out longhand at eight call sites in the room service,
//! each repeating the cache-miss fallback, and each free to get it subtly wrong — one of them read
//! only the participant ids, another loaded full members, a third skipped the cache entirely.
//!
//! `RoomNotifier` is that pair, once.

use crate::broadcast::{BroadcastChannel, NotificationEvent};
use crate::cache::redis_cache::Cache;
use crate::core::errors::AppError;
use crate::rooms::model::RoomContext;
use crate::rooms::repository::RoomRepository;
use std::sync::Arc;
use uuid::Uuid;

/// Resolves room membership and broadcasts to it.
///
/// Shared by cloning — the `Arc`s inside are the shared state, the struct itself is a handle.
#[derive(Clone)]
pub struct RoomNotifier {
    bus: Arc<BroadcastChannel>,
    rooms: RoomRepository,
    cache: Arc<dyn Cache>,
}

impl RoomNotifier {
    pub fn new(bus: Arc<BroadcastChannel>, rooms: RoomRepository, cache: Arc<dyn Cache>) -> Self {
        Self { bus, rooms, cache }
    }

    /// The room's current participants, from cache when possible.
    ///
    /// On a miss the members are read from PostgreSQL and written back, so a busy room costs one
    /// query per cache lifetime rather than one per message. With `NoOpCache` every call is a
    /// miss and this degrades to a plain query — correct, just not cached.
    pub async fn room_context(&self, room_id: &Uuid) -> Result<RoomContext, AppError> {
        if let Some(context) = self.cache.get_room_context(room_id).await? {
            return Ok(context);
        }

        let context = RoomContext::from_rows(self.rooms.select_all_room_member(room_id).await?);
        self.cache.set_room_context(room_id, &context).await?;
        Ok(context)
    }

    /// Drops the cached participant snapshot.
    ///
    /// Call this after any write that changes who is in the room — join, leave, invite — and
    /// **before** broadcasting, so a listener that reacts by reading the room does not race a
    /// stale snapshot.
    pub async fn invalidate(&self, room_id: &Uuid) -> Result<(), AppError> {
        self.cache.invalidate_room_context(room_id).await?;
        Ok(())
    }

    /// Broadcasts an event to everyone currently in the room.
    ///
    /// Returns `Err` only when membership could not be resolved; a recipient being offline is not
    /// a failure — the bus falls back to a push notification for the event types that warrant one.
    pub async fn notify_room(&self, room_id: &Uuid, event: NotificationEvent) -> Result<(), AppError> {
        let context = self.room_context(room_id).await?;
        self.bus.notify_all(context.member_ids(), event).await;
        Ok(())
    }

    /// Broadcasts to an explicit recipient list.
    ///
    /// Used when the audience is *not* the room's current membership: the user who just left (and
    /// is therefore no longer a member), or users being invited to a room they have not joined.
    pub async fn notify_users(&self, user_ids: Vec<Uuid>, event: NotificationEvent) {
        self.bus.notify_all(user_ids, event).await;
    }

    /// Broadcasts to a single user.
    pub async fn notify_user(&self, user_id: &Uuid, event: NotificationEvent) {
        self.bus.notify(user_id, event).await;
    }
}
