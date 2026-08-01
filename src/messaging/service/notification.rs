//! Subscription lifecycle and replay for the live notification stream.
//!
//! The SSE and WebSocket handlers used to own all of this: they reached for the
//! `BroadcastChannel` global, read `Arc<dyn Cache>` straight off `AppState`, and each carried its
//! own copy of the handshake rules. The transport-independent half — who is subscribed, what has
//! to be replayed, when a client must resync — lives here now, so both endpoints answer those
//! questions the same way and neither touches the cache directly.

use crate::broadcast::{BroadcastChannel, Notification, NotificationEvent};
use crate::cache::redis_cache::{Cache, ReplayResult};
use crate::core::errors::AppError;
use crate::core::{Service, ShutdownSignal};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
use tracing::error;
use uuid::Uuid;

/// Live-stream subscriptions and durable-event replay.
#[derive(Clone)]
pub struct NotificationService {
    bus: Arc<BroadcastChannel>,
    cache: Arc<dyn Cache>,
    shutdown: ShutdownSignal,
}

impl Service for NotificationService {
    const NAME: &'static str = "NotificationService";
}

impl NotificationService {
    pub fn new(bus: Arc<BroadcastChannel>, cache: Arc<dyn Cache>, shutdown: ShutdownSignal) -> Self {
        Self { bus, cache, shutdown }
    }

    /// Resolves when the server has begun shutting down.
    ///
    /// Both streaming endpoints must select on this. Axum cannot end a live connection for them:
    /// its graceful shutdown only asks HTTP/1 to finish the in-flight response, and an SSE body
    /// never finishes while an upgraded WebSocket is past the HTTP layer altogether. A stream that
    /// ignores this signal holds the whole process open until it is killed.
    ///
    /// The signal lives on this service rather than being its own `FromRef` extractor because this
    /// is already where a connection's lifecycle is decided — [`Self::subscribe`],
    /// [`Self::connection_guard`], [`Self::resolve_handshake`].
    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + use<> {
        self.shutdown.cancelled()
    }

    /// Registers a connection and returns its event receiver.
    ///
    /// Subscribe **before** resolving the handshake: events produced while the replay is being
    /// read are then buffered by the receiver instead of falling into the gap between the two.
    pub async fn subscribe(&self, user_id: Uuid) -> Receiver<Notification> {
        self.bus.subscribe_to_user_events(user_id).await
    }

    /// Guard that unsubscribes the user when the connection's stream is dropped.
    pub fn connection_guard(&self, user_id: Uuid) -> ConnectionGuard {
        ConnectionGuard {
            bus: self.bus.clone(),
            user_id,
        }
    }

    /// Resolves the connection handshake into (events to replay first, high-water sequence).
    ///
    /// The high-water sequence is the largest sequence the client is guaranteed to have after the
    /// replay; live events with a sequence `<= high_water` are duplicates and get filtered out.
    /// A returned `Resync` event sets the high-water back to 0 so the client receives every
    /// subsequent live event while it reloads state out-of-band.
    pub async fn resolve_handshake(&self, user_id: &Uuid, last_seq: Option<u64>) -> (Vec<Notification>, u64) {
        let last_seq = match last_seq {
            Some(seq) => seq,
            None => return (vec![], 0), // fresh connection: nothing to replay
        };

        match self.bus.replay_since(user_id, last_seq).await {
            Ok(ReplayResult::Events(events)) => {
                let high_water = events.iter().filter_map(|n| n.seq).max().unwrap_or(last_seq);
                (events, high_water)
            }
            Ok(ReplayResult::ResyncNeeded) => (vec![Self::resync(HISTORY_UNAVAILABLE)], 0),
            Err(err) => {
                error!(%user_id, error = %err, "Failed to fetch notification replay");
                (vec![Self::resync("replay error, please resync via REST")], 0)
            }
        }
    }

    /// Highest sequence issued to a user so far, without advancing it.
    ///
    /// `0` when nothing has been issued yet, and also when sequencing is unavailable (no Redis) —
    /// a client that gets `0` simply has no replay baseline, which is the correct behaviour in
    /// both cases.
    pub async fn current_sequence(&self, user_id: &Uuid) -> Result<u64, AppError> {
        Ok(self.cache.current_sequence(user_id).await?.unwrap_or(0))
    }

    /// Durable events after `last_seq`, or a single `Resync` if the gap has been trimmed away.
    pub async fn events_since(&self, user_id: &Uuid, last_seq: u64) -> Result<Vec<Notification>, AppError> {
        let events = match self.cache.get_notifications_since_seq(user_id, last_seq).await? {
            ReplayResult::Events(events) => events,
            ReplayResult::ResyncNeeded => vec![Self::resync(HISTORY_UNAVAILABLE)],
        };
        Ok(events)
    }

    /// Control notification telling the client its cached history is unavailable and it must
    /// re-fetch authoritative state via REST.
    pub fn resync(reason: &str) -> Notification {
        Notification::new(NotificationEvent::Resync { reason: reason.to_string() })
    }
}

const HISTORY_UNAVAILABLE: &str = "history unavailable, please resync via REST";

/// Unsubscribes a user when their connection ends.
///
/// Holds the bus rather than reaching for a global, so the cleanup is tied to the same instance
/// the connection subscribed to. `Drop` cannot be async, hence the detached task.
pub struct ConnectionGuard {
    bus: Arc<BroadcastChannel>,
    user_id: Uuid,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let bus = self.bus.clone();
        let user_id = self.user_id;
        tokio::spawn(async move {
            bus.unsubscribe(user_id).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::test_support::{FailingCache, InMemoryCache};
    use crate::core::{KafkaConfig, ShutdownController};
    use crate::kafka::PushNotificationProducer;

    fn service(cache: Arc<dyn Cache>) -> NotificationService {
        let producer = PushNotificationProducer::connect(
            false,
            KafkaConfig {
                bootstrap_host: String::new(),
                bootstrap_port: 0,
                topic: String::new(),
                client_id: String::new(),
                partition: vec![],
                consumer_group: String::new(),
            },
        )
        .expect("logging producer never fails");

        let bus = Arc::new(BroadcastChannel::new(cache.clone(), producer));
        NotificationService::new(bus, cache, ShutdownController::new().signal())
    }

    fn is_resync(notification: &Notification) -> bool {
        matches!(notification.body, NotificationEvent::Resync { .. })
    }

    /// A fresh connection has no cursor, so there is nothing to replay and no baseline to dedupe
    /// against — every live event must reach it.
    #[tokio::test]
    async fn a_connection_without_a_cursor_replays_nothing() {
        let (events, high_water) = service(Arc::new(InMemoryCache::new())).resolve_handshake(&Uuid::new_v4(), None).await;

        assert!(events.is_empty());
        assert_eq!(high_water, 0);
    }

    #[tokio::test]
    async fn a_reconnecting_client_gets_the_events_it_missed() {
        let cache = Arc::new(InMemoryCache::new());
        let user_id = Uuid::new_v4();

        for _ in 0..3 {
            cache
                .append_notification(&user_id, &NotificationService::resync("filler"))
                .await
                .expect("append");
        }

        let (events, high_water) = service(cache).resolve_handshake(&user_id, Some(1)).await;

        assert_eq!(events.iter().filter_map(|event| event.seq).collect::<Vec<_>>(), vec![2, 3]);
        // The client is now guaranteed to hold everything up to 3; live events at or below that
        // are duplicates.
        assert_eq!(high_water, 3);
    }

    /// The gap fell out of the retained window. The client cannot be caught up losslessly, so it is
    /// told to reload via REST — and the high-water goes back to 0 so nothing that arrives during
    /// the reload is filtered out as a duplicate.
    #[tokio::test]
    async fn a_gap_that_was_trimmed_away_forces_a_resync() {
        let cache = Arc::new(InMemoryCache::new());
        let user_id = Uuid::new_v4();

        for _ in 0..5 {
            cache
                .append_notification(&user_id, &NotificationService::resync("filler"))
                .await
                .expect("append");
        }
        cache.trim_to(&user_id, 2); // only seq 4 and 5 survive; a client at 1 cannot be served

        let (events, high_water) = service(cache).resolve_handshake(&user_id, Some(1)).await;

        assert_eq!(events.len(), 1);
        assert!(is_resync(&events[0]));
        assert_eq!(high_water, 0);
    }

    /// The cursor is ahead of the counter, so the sequence space was reset under the client.
    #[tokio::test]
    async fn a_cursor_ahead_of_the_counter_forces_a_resync() {
        let (events, high_water) = service(Arc::new(InMemoryCache::new())).resolve_handshake(&Uuid::new_v4(), Some(999)).await;

        assert_eq!(events.len(), 1);
        assert!(is_resync(&events[0]));
        assert_eq!(high_water, 0);
    }

    /// An unreachable cache must not silently look like "nothing to replay": that would leave the
    /// client believing it is caught up.
    #[tokio::test]
    async fn an_unreachable_cache_forces_a_resync() {
        let (events, high_water) = service(Arc::new(FailingCache)).resolve_handshake(&Uuid::new_v4(), Some(1)).await;

        assert_eq!(events.len(), 1);
        assert!(is_resync(&events[0]));
        assert_eq!(high_water, 0);
    }
}
