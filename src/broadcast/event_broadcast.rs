use crate::broadcast::{Notification, NotificationEvent};
use crate::cache::redis_cache::{Cache, ReplayResult};
use crate::kafka::{EventProducer, PushNotificationProducer};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::sync::broadcast::{Receiver, Sender, channel};
use tracing::{debug, error, warn};
use uuid::Uuid;

/// How many recipients of one fan-out are sequenced, cached and delivered at a time.
///
/// Each costs a Redis round trip on the multiplexed [`ConnectionManager`] plus a clone of the
/// envelope. Unbounded concurrency would let one large room fire thousands of simultaneous commands
/// down the connection every other request shares, and hold a clone of the message for each. 32
/// turns a 50-member room into two batches while bounding both.
///
/// [`ConnectionManager`]: redis::aio::ConnectionManager
const FANOUT_CONCURRENCY: usize = 32;

/// Recipients per push-notification record. One record for the whole offline set is the point of
/// batching, but the record grows with the audience — 5000 UUIDs is already ~180 KB against
/// Kafka's 1 MB default `message.max.bytes`. Chunking is a loop; trusting rooms to stay small is a
/// bet.
const PUSH_BATCH_SIZE: usize = 500;

/// A fan-out slower than this is logged at `warn`, so the pathological cases stay visible at the
/// production log level without anyone turning on `debug`.
const SLOW_FANOUT: Duration = Duration::from_millis(250);

/// Outcome of one recipient's delivery. `Offline` is not a failure: it is what the fan-out collects
/// into the single batched push notification.
enum Delivery {
    Live,
    Offline,
}

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
    /// Prefer this over [`Self::send_event_to_all`]: taking the [`NotificationEvent`] rather than a
    /// pre-built [`Notification`] means the envelope is constructed in exactly one place, so the
    /// version field and the unset `seq` cannot be got wrong at a call site.
    pub async fn notify(&self, to_user: &Uuid, event: NotificationEvent) {
        self.send_event_to_all(vec![*to_user], Notification::new(event)).await;
    }

    /// Sends one event to many users. Each recipient gets its own `seq` — see
    /// [`Self::send_event_to_all`].
    pub async fn notify_all(&self, user_ids: Vec<Uuid>, event: NotificationEvent) {
        self.send_event_to_all(user_ids, Notification::new(event)).await;
    }

    /// Sends an already-built envelope to many users. Prefer [`Self::notify_all`], which builds the
    /// envelope for you.
    ///
    /// Recipients are independent — each touches only its own Redis keys and its own broadcast
    /// sender — so they are delivered concurrently, bounded by [`FANOUT_CONCURRENCY`]. Per-user
    /// ordering is unaffected: a user appears at most once in a fan-out, and the whole fan-out is
    /// awaited, so two successive calls cannot interleave.
    ///
    /// Offline recipients are collected and pushed in **one** Kafka record rather than one each.
    pub async fn send_event_to_all(&self, user_ids: Vec<Uuid>, notification: Notification) {
        let ephemeral = notification.body.is_ephemeral();
        let recipients = user_ids.len();
        let started = Instant::now();

        // A sequence number is per-user, so every recipient gets its own clone with its own seq
        // rather than a single shared notification.
        let offline: Vec<Uuid> = futures::stream::iter(user_ids)
            .map(|user_id| {
                let notification = notification.clone();
                async move {
                    match self.deliver_to_user(&user_id, notification).await {
                        Delivery::Live => None,
                        Delivery::Offline => Some(user_id),
                    }
                }
            })
            .buffer_unordered(FANOUT_CONCURRENCY)
            .filter_map(std::future::ready)
            .collect()
            .await;

        // Measured before the push, so the number reflects the fan-out itself.
        let elapsed = started.elapsed();
        let offline_count = offline.len();

        if !ephemeral && !offline.is_empty() {
            self.send_undeliverable_notifications(notification, offline).await;
        }

        let duration_ms = elapsed.as_millis() as u64;
        if elapsed >= SLOW_FANOUT {
            warn!(recipients, offline = offline_count, duration_ms, "Slow notification fan-out");
        } else {
            debug!(recipients, offline = offline_count, duration_ms, "Notification fan-out complete");
        }
    }

    /// Deliver a single notification to a single user.
    ///
    /// Durable events are sequenced and cached for replay in one atomic Redis call before
    /// delivery; ephemeral events (typing, resync signals) are sent live-only. Reports whether a
    /// live connection took it — the push fallback belongs to the caller, which batches every
    /// offline recipient of a fan-out into one record.
    async fn deliver_to_user(&self, user_id: &Uuid, mut notification: Notification) -> Delivery {
        if !notification.body.is_ephemeral() {
            match self.cache.append_notification(user_id, &notification).await {
                // Sequencing available (Redis): the event is now durable under this seq.
                Ok(Some(seq)) => notification.seq = Some(seq),
                // No sequencing (no Redis): deliver best-effort without replay support.
                Ok(None) => {}
                // Deliberately delivered with no seq: a number we failed to store is not
                // replayable, and handing it out would advance the client's cursor past an event
                // that is not in the stream.
                Err(error) => error!(%user_id, error = %error, "Failed to sequence and cache notification"),
            }
        }

        let lock = self.channel.read().await;
        match lock.get(user_id).map(|sender| sender.send(notification)) {
            Some(Ok(receivers)) => {
                debug!(%user_id, receivers, "Broadcast event delivered");
                Delivery::Live
            }
            // `send` only fails when nobody is listening, i.e. the user is offline. That is
            // expected, not an error: the caller's push-notification batch picks it up.
            Some(Err(_)) | None => {
                debug!(%user_id, "No active receiver for notification");
                Delivery::Offline
            }
        }
    }

    async fn send_undeliverable_notifications(&self, mut notification: Notification, to_user: Vec<Uuid>) {
        let should_send = matches!(
            //Only sends push notifications for these notification types, add more if needed
            notification.body,
            NotificationEvent::ChatMessage { .. } | NotificationEvent::FriendRequestReceived { .. } | NotificationEvent::NewRoom { .. }
        );

        if !should_send {
            return;
        }

        // One envelope, many recipients, and `seq` is per-user — there is no single correct value,
        // so it is omitted rather than picked. The push consumer renders the event; a client that
        // reconnects replays from its own stored cursor. `seq` is already absent on this wire
        // whenever ISM runs without Redis, so its absence is nothing new for the consumer.
        notification.seq = None;

        for chunk in to_user.chunks(PUSH_BATCH_SIZE) {
            let recipients = chunk.len();
            if let Err(error) = self.push_notification_producer.send_notification(notification.clone(), chunk.to_vec()).await {
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
    use crate::cache::redis_cache::{Cache, NoOpCache};
    use crate::cache::test_support::InMemoryCache;
    use crate::core::KafkaConfig;
    use crate::kafka::{PushNotificationProducer, RecordingEventProducer};
    use crate::users::response::UserProfileResponse;
    use std::sync::Arc;

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

    fn logging_producer() -> PushNotificationProducer {
        PushNotificationProducer::connect(false, empty_kafka_cfg()).expect("logging producer never fails")
    }

    fn read_receipt(user_id: Uuid) -> Notification {
        Notification::new(UserReadChat {
            user_id,
            room_id: Uuid::new_v4(),
        })
    }

    /// A minimal profile, for the events whose payload is not what the test is about.
    fn profile(id: Uuid) -> UserProfileResponse {
        UserProfileResponse {
            id,
            display_name: String::from("Test User"),
            street_credits: 0,
            profile_picture: None,
            description: None,
            friends_count: 0,
            posts_count: 0,
            role: String::from("User"),
        }
    }

    #[tokio::test]
    async fn send_event_to_subscribed_user_delivers_notification() {
        // A bus of its own, with a NoOpCache and the logging producer. This used to initialise a
        // process-wide singleton, which meant the two tests in this module shared one instance and
        // whichever ran first decided its cache.
        let cache: Arc<dyn Cache> = Arc::new(NoOpCache);
        let bc = BroadcastChannel::new(cache, logging_producer());

        let user_id = Uuid::new_v4();
        // subscribe
        let mut rx = bc.subscribe_to_user_events(user_id).await;

        let notification = read_receipt(user_id);

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

    /// The fan-out runs recipients concurrently, so the thing worth pinning is that concurrency
    /// neither drops nor duplicates anyone. Deliberately more recipients than `FANOUT_CONCURRENCY`,
    /// so more than one batch runs.
    #[tokio::test]
    async fn concurrent_fan_out_reaches_every_recipient_exactly_once() {
        let recipients = FANOUT_CONCURRENCY * 2 - 4;

        let cache = Arc::new(InMemoryCache::new());
        let bc = BroadcastChannel::new(cache.clone(), logging_producer());

        let mut receivers = Vec::with_capacity(recipients);
        let mut user_ids = Vec::with_capacity(recipients);
        for _ in 0..recipients {
            let user_id = Uuid::new_v4();
            receivers.push((user_id, bc.subscribe_to_user_events(user_id).await));
            user_ids.push(user_id);
        }

        bc.notify_all(
            user_ids,
            UserReadChat {
                user_id: Uuid::new_v4(),
                room_id: Uuid::new_v4(),
            },
        )
        .await;

        for (user_id, mut rx) in receivers {
            let received = rx.recv().await.unwrap_or_else(|err| panic!("{user_id} received nothing: {err}"));
            // Each recipient has its own sequence space, so every one of them sees its first event.
            assert_eq!(received.seq, Some(1), "{user_id} got the wrong sequence");
            assert!(rx.try_recv().is_err(), "{user_id} received the event twice");
        }

        assert_eq!(cache.cached_count(), recipients);
    }

    /// The offline recipients of one fan-out share a single push record. Before this, a 50-member
    /// room with 50 offline members produced 50 Kafka records.
    #[tokio::test]
    async fn offline_recipients_share_one_batched_push() {
        let recorder = Arc::new(RecordingEventProducer::new());
        let bc = BroadcastChannel::new(Arc::new(InMemoryCache::new()), PushNotificationProducer::Recording(recorder.clone()));

        let online: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let offline: Vec<Uuid> = (0..7).map(|_| Uuid::new_v4()).collect();

        // Keep the receivers alive: a dropped receiver makes the user look offline.
        let mut _receivers = Vec::new();
        for user_id in &online {
            _receivers.push(bc.subscribe_to_user_events(*user_id).await);
        }

        bc.notify_all(
            online.iter().chain(offline.iter()).copied().collect(),
            NotificationEvent::FriendRequestReceived {
                from_user: profile(Uuid::new_v4()),
            },
        )
        .await;

        let sent = recorder.sent();
        assert_eq!(sent.len(), 1, "expected one batched record, got {}", sent.len());

        let (notification, pushed_to) = &sent[0];
        assert_eq!(pushed_to.len(), offline.len());
        for user_id in &offline {
            assert!(pushed_to.contains(user_id), "{user_id} was offline but not pushed to");
        }
        for user_id in &online {
            assert!(!pushed_to.contains(user_id), "{user_id} was online but still pushed to");
        }
        // One envelope for many recipients cannot carry a per-user sequence.
        assert_eq!(notification.seq, None);
    }

    /// Ephemeral events are live-only in both directions: no sequence, no cache entry, and no push
    /// for the recipients that were offline.
    #[tokio::test]
    async fn ephemeral_events_are_never_pushed() {
        let recorder = Arc::new(RecordingEventProducer::new());
        let cache = Arc::new(InMemoryCache::new());
        let bc = BroadcastChannel::new(cache.clone(), PushNotificationProducer::Recording(recorder.clone()));

        bc.notify_all(vec![Uuid::new_v4(), Uuid::new_v4()], NotificationEvent::Resync { reason: "too old".into() })
            .await;

        assert!(recorder.sent().is_empty());
        assert_eq!(cache.cached_count(), 0);
    }
}
