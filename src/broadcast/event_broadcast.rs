use crate::broadcast::{Notification, NotificationEvent};
use crate::cache::redis_cache::{Cache, ReplayResult};
use crate::kafka::{EventProducer, PushNotificationProducer};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::broadcast::{Receiver, Sender, channel};
use tracing::{debug, error};
use uuid::Uuid;

/// A `BroadcastChannel` struct is responsible for managing a collection of channels that are used
/// for broadcasting notifications to subscribers. Each channel is uniquely identified by a `Uuid`,
/// and messages are sent through a `Sender<Notification>`.
///
/// The struct uses an `RwLock` for thread-safe, concurrent access to the underlying `HashMap`.
///
/// # Fields
/// - `channel`: An `RwLock`-protected `HashMap` that maps a `Uuid` (unique identifier) to a `Sender<Notification>`.
///   - `Uuid`: A unique identifier for each channel.
///   - `Sender<Notification>`: A sender handle for sending `Notification` messages to the corresponding receiver.
///
/// The `BroadcastChannel` is designed to support multi-threaded operations where multiple threads
/// may add, retrieve, or remove channels or broadcast messages safely.
///
///
/// # Thread Safety
/// The usage of `RwLock` ensures that the operations on the `HashMap` are synchronized
/// and can safely be used across multiple threads. Readers can access the map concurrently,
/// while write operations are exclusive to ensure data integrity.
pub struct BroadcastChannel {
    channel: UserConnectionMap,
    cache: Arc<dyn Cache>,
    push_notification_producer: PushNotificationProducer,
}

type UserConnectionMap = RwLock<HashMap<Uuid, Sender<Notification>>>;

impl BroadcastChannel {
    /// Builds the bus.
    ///
    /// Called once, by [`AppStateBuilder`](crate::core::AppStateBuilder), which then hands an
    /// `Arc<BroadcastChannel>` to every service and background task that needs it. There is no
    /// global: a task gets its own `Arc` clone moved in at the `tokio::spawn` site, which keeps
    /// the dependency visible and lets a test build a bus of its own.
    pub fn new(cache: Arc<dyn Cache>, producer: PushNotificationProducer) -> Self {
        BroadcastChannel {
            channel: RwLock::new(HashMap::new()),
            push_notification_producer: producer,
            cache,
        }
    }

    pub async fn subscribe_to_user_events(&self, user_id: Uuid) -> Receiver<Notification> {
        let mut lock = self.channel.write().await;
        let sender = lock.entry(user_id).or_insert_with(|| channel::<Notification>(100).0);
        sender.subscribe()
    }

    /// Replay durable notifications for a user with sequence greater than `last_seq`. Used by
    /// the SSE/WebSocket handshake so a reconnecting client can catch up without losing events.
    pub async fn replay_since(&self, user_id: &Uuid, last_seq: u64) -> redis::RedisResult<ReplayResult> {
        self.cache.get_notifications_since_seq(user_id, last_seq).await
    }

    /// Sends one event to one user.
    ///
    /// Prefer this over [`Self::send_event`]: taking the [`NotificationEvent`] rather than a
    /// pre-built [`Notification`] means the envelope is constructed in exactly one place, so the
    /// version field and the unset `seq` cannot be got wrong at a call site.
    pub async fn notify(&self, to_user: &Uuid, event: NotificationEvent) {
        self.deliver_to_user(to_user, Notification::new(event)).await;
    }

    /// Sends one event to many users. Each recipient gets its own `seq` — see
    /// [`Self::send_event_to_all`].
    pub async fn notify_all(&self, user_ids: Vec<Uuid>, event: NotificationEvent) {
        self.send_event_to_all(user_ids, Notification::new(event)).await;
    }

    /// Sends an already-built envelope. Used when the envelope did not originate here — the Redis
    /// subscriber forwards notifications that were serialized by another node and must keep their
    /// original `createdAt`. Everywhere else, prefer [`Self::notify`].
    pub async fn send_event(&self, notification: Notification, to_user: &Uuid) {
        self.deliver_to_user(to_user, notification).await;
    }

    /// Fan-out counterpart of [`Self::send_event`]. Prefer [`Self::notify_all`].
    pub async fn send_event_to_all(&self, user_ids: Vec<Uuid>, notification: Notification) {
        // A sequence number is per-user, so every recipient gets its own clone with its own
        // seq rather than a single shared notification.
        for user_id in user_ids {
            self.deliver_to_user(&user_id, notification.clone()).await;
        }
    }

    /// Deliver a single notification to a single user.
    ///
    /// Durable events are assigned a per-user sequence number and cached for replay before
    /// delivery; ephemeral events (typing, resync signals) are sent live-only. If the user has
    /// no active connection, durable events fall back to a push notification.
    async fn deliver_to_user(&self, user_id: &Uuid, mut notification: Notification) {
        let ephemeral = notification.body.is_ephemeral();

        if !ephemeral {
            match self.cache.next_sequence(user_id).await {
                // Sequencing available (Redis): tag the event and cache it for replay.
                Ok(Some(seq)) => {
                    notification.seq = Some(seq);
                    if let Err(error) = self.cache.add_notification_for_user(user_id, &notification).await {
                        error!(%user_id, error = %error, "Failed to cache notification");
                    }
                }
                // No sequencing (no Redis): deliver best-effort without replay support.
                Ok(None) => {}
                Err(err) => {
                    error!(%user_id, error = %err, "Failed to allocate notification sequence")
                }
            }
        }

        let delivered = {
            let lock = self.channel.read().await;
            match lock.get(user_id) {
                // `send` only errors when there are no active receivers, i.e. the user is offline.
                Some(sender) => match sender.send(notification.clone()) {
                    Ok(sc) => {
                        debug!(%user_id, receivers = sc, "Broadcast event delivered");
                        true
                    }
                    Err(err) => {
                        // `send` only fails when nobody is listening, i.e. the user is offline.
                        // That is expected, not an error: the push-notification path below picks
                        // it up.
                        debug!(%user_id, error = %err, "No active receiver for notification");
                        false
                    }
                },
                None => false,
            }
        };

        if !delivered && !ephemeral {
            self.send_undeliverable_notifications(notification, vec![*user_id]).await;
        }
    }

    async fn send_undeliverable_notifications(&self, notification: Notification, to_user: Vec<Uuid>) {
        let should_send = matches!(
            //Only sends push notifications for these notification types, add more if needed
            notification.body,
            NotificationEvent::ChatMessage { .. } | NotificationEvent::FriendRequestReceived { .. } | NotificationEvent::NewRoom { .. }
        );

        if should_send {
            let recipients = to_user.len();
            if let Err(error) = self.push_notification_producer.send_notification(notification, to_user).await {
                error!(recipients, error = %error, "Failed to send push notification");
            }
        }
    }

    pub async fn unsubscribe(&self, user_id: Uuid) {
        debug!(%user_id, "Unsubscribing user from broadcast events");
        let mut lock = self.channel.write().await;
        if let Some(sender) = lock.get(&user_id) {
            if sender.receiver_count() > 0 {
                return;
            } else {
                lock.remove(&user_id);
                debug!(%user_id, "Removed stale broadcast sender");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcast::Notification;
    use crate::broadcast::NotificationEvent;
    use crate::broadcast::NotificationEvent::UserReadChat;
    use crate::cache::redis_cache::{Cache, NoOpCache, ReplayResult};
    use crate::core::KafkaConfig;
    use crate::kafka::PushNotificationProducer;
    use crate::rooms::model::RoomContext;
    use async_trait::async_trait;
    use redis::RedisResult;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn empty_kafka_cfg() -> KafkaConfig {
        KafkaConfig {
            bootstrap_host: String::from(""),
            bootstrap_port: 0,
            topic: String::from(""),
            client_id: String::from(""),
            partition: vec![],
            consumer_group: String::from(""),
        }
    }

    /// In-memory `Cache` used to exercise the broadcast layer without Redis: it hands out a
    /// real monotonic per-user sequence and records everything that gets cached.
    struct MockCache {
        sequences: Mutex<HashMap<Uuid, u64>>,
        cached: Mutex<Vec<(Uuid, Notification)>>,
    }

    impl MockCache {
        fn new() -> Self {
            MockCache {
                sequences: Mutex::new(HashMap::new()),
                cached: Mutex::new(Vec::new()),
            }
        }
        fn cached_count(&self) -> usize {
            self.cached.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl Cache for MockCache {
        async fn next_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>> {
            let mut seqs = self.sequences.lock().unwrap();
            let entry = seqs.entry(*user_id).or_insert(0);
            *entry += 1;
            Ok(Some(*entry))
        }
        async fn current_sequence(&self, user_id: &Uuid) -> RedisResult<Option<u64>> {
            let seqs = self.sequences.lock().unwrap();
            Ok(Some(seqs.get(user_id).copied().unwrap_or(0)))
        }
        async fn get_notifications_since_seq(&self, user_id: &Uuid, last_seq: u64) -> RedisResult<ReplayResult> {
            let cached = self.cached.lock().unwrap();
            let events = cached
                .iter()
                .filter(|(uid, n)| uid == user_id && n.seq.map_or(false, |s| s > last_seq))
                .map(|(_, n)| n.clone())
                .collect();
            Ok(ReplayResult::Events(events))
        }
        async fn add_notification_for_user(&self, user_id: &Uuid, notification: &Notification) -> RedisResult<()> {
            self.cached.lock().unwrap().push((*user_id, notification.clone()));
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

    #[tokio::test]
    async fn send_event_to_subscribed_user_delivers_notification() {
        // A bus of its own, with a NoOpCache and the logging producer. This used to initialise a
        // process-wide singleton, which meant the two tests in this module shared one instance and
        // whichever ran first decided its cache.
        let cache: Arc<dyn Cache> = Arc::new(NoOpCache);
        let bc = BroadcastChannel::new(
            cache,
            PushNotificationProducer::connect(false, empty_kafka_cfg()).expect("logging producer never fails"),
        );

        let user_id = uuid::Uuid::new_v4();
        // subscribe
        let mut rx = bc.subscribe_to_user_events(user_id).await;

        let notification = Notification::new(UserReadChat {
            user_id,
            room_id: uuid::Uuid::new_v4(),
        });

        // send to all (only this user)
        bc.send_event_to_all(vec![user_id], notification.clone()).await;

        // receive
        let received = rx.recv().await.expect("Should receive notification");

        // Without Redis there is no sequencing, so the delivered event matches what was sent.
        let sent_json = serde_json::to_string(&notification).expect("serialize sent");
        let recv_json = serde_json::to_string(&received).expect("serialize recv");
        assert_eq!(sent_json, recv_json);
        assert_eq!(received.seq, None);
    }

    #[tokio::test]
    async fn assigns_independent_per_user_sequence_and_skips_ephemeral() {
        let cache = Arc::new(MockCache::new());
        let bc = BroadcastChannel::new(
            cache.clone(),
            PushNotificationProducer::connect(false, empty_kafka_cfg()).expect("logging producer never fails"),
        );

        let user_a = Uuid::new_v4();
        let mut rx_a = bc.subscribe_to_user_events(user_a).await;

        // Two durable events to the same user -> monotonic seq 1, then 2.
        bc.send_event(
            Notification::new(UserReadChat {
                user_id: user_a,
                room_id: Uuid::new_v4(),
            }),
            &user_a,
        )
        .await;
        bc.send_event(
            Notification::new(UserReadChat {
                user_id: user_a,
                room_id: Uuid::new_v4(),
            }),
            &user_a,
        )
        .await;
        assert_eq!(rx_a.recv().await.expect("a1").seq, Some(1));
        assert_eq!(rx_a.recv().await.expect("a2").seq, Some(2));

        // A second user has an independent sequence space (also starts at 1).
        let user_b = Uuid::new_v4();
        let mut rx_b = bc.subscribe_to_user_events(user_b).await;
        bc.send_event(
            Notification::new(UserReadChat {
                user_id: user_b,
                room_id: Uuid::new_v4(),
            }),
            &user_b,
        )
        .await;
        assert_eq!(rx_b.recv().await.expect("b1").seq, Some(1));

        // Ephemeral event: no sequence number, never cached.
        bc.send_event(Notification::new(NotificationEvent::Resync { reason: "too old".into() }), &user_a)
            .await;
        assert_eq!(rx_a.recv().await.expect("resync").seq, None);

        // Only the 3 durable events were cached; the ephemeral one was not.
        assert_eq!(cache.cached_count(), 3);
    }
}
