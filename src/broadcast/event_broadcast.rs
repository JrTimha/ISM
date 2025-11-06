use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use log::{debug, error, info};
use tokio::sync::{OnceCell, RwLock};
use uuid::Uuid;
use tokio::sync::broadcast::{Sender, channel, Receiver};
use tokio::time::interval;
use crate::broadcast::Notification;
use crate::cache::redis_cache::Cache;

static BROADCAST_INSTANCE: OnceCell<Arc<BroadcastChannel>> = OnceCell::const_new();

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
    channel: RwLock<HashMap<Uuid, Sender<Notification>>>,
    notification_cache: Arc<dyn Cache>
}

impl BroadcastChannel {

    pub async fn init(cache: Arc<dyn Cache>) {
        BROADCAST_INSTANCE.get_or_init(|| async {
            let channel = Arc::new(BroadcastChannel::new(cache));
            //channel.clone().start_cleanup_task();
            info!("BroadcastChannel initialized.");
            channel
        }).await;
    }

    pub fn get() -> &'static Arc<BroadcastChannel> {
        match BROADCAST_INSTANCE.get() {
            None => {
                panic!("BroadcastChannel is not initialized! Call init()!");
            }
            Some(instance) => instance
        }
    }

    fn new(cache: Arc<dyn Cache>) -> Self {
        BroadcastChannel {
            channel: RwLock::new(HashMap::new()),
            notification_cache: cache
        }
    }

    #[allow(dead_code)]
    fn start_cleanup_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                debug!("Starting broadcast garbage collection");
                self.cleanup_senders().await;
            }
        });
    }

    async fn cleanup_senders(&self) {
        let mut lock = self.channel.write().await;
        lock.retain(|&user_id, sender| {
            if sender.receiver_count() > 0 {
                true
            } else {
                info!("Removing stale sender for user {:?}", user_id);
                false
            }
        });
    }

    pub async fn subscribe_to_user_events(&self, user_id: Uuid) -> Receiver<Notification> {
        let mut lock = self.channel.write().await;
        let sender = lock.entry(user_id)
            .or_insert_with(|| channel::<Notification>(100).0);
        sender.subscribe()
    }

    pub async fn send_event(&self, notification: Notification, to_user: &Uuid) {
        let lock = self.channel.read().await;
        if let Some(sender) = lock.get(to_user) {
            match sender.send(notification) {
                Ok(sc) => {
                    info!("Successfully sent {:?} broadcast event.", sc);
                }
                Err(err) => {
                    error!("Unable to broadcast notification: {}", err);
                }
            }
        }
    }

    pub async fn send_event_to_all(&self, user_ids: Vec<Uuid>, notification: Notification) {
        let lock = self.channel.read().await;
        for user_id in user_ids {
            if let Some(sender) = lock.get(&user_id) {
                match sender.send(notification.clone()) {
                    Ok(sc) => {
                        info!("Successfully sent {:?} broadcast event.", sc);
                    }
                    Err(err) => {
                        error!("Unable to broadcast notification: {}", err);
                    }
                }
            } else {
                if let Err(error) = self.notification_cache.add_notification_for_user(&user_id, &notification).await {
                    error!("Failed to cache notification: {}", error);
                };
            }
        }
    }
    

    pub async fn unsubscribe(&self, user_id: Uuid) {
        let mut lock = self.channel.write().await;
        if let Some(sender) = lock.get(&user_id) {
            if sender.receiver_count() > 0 {
                return
            } else {
                lock.remove(&user_id);
                debug!("Removed stale sender for user {:?}", user_id);
            }
        }
    }

}
