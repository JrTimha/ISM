use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use log::{debug, error, info};
use tokio::sync::{OnceCell, RwLock};
use uuid::Uuid;
use tokio::sync::broadcast::{Sender, channel, Receiver};
use tokio::time::interval;
use crate::broadcast::Notification;

static BROADCAST_INSTANCE: OnceCell<Arc<BroadcastChannel>> = OnceCell::const_new();

pub struct BroadcastChannel {
    channel: RwLock<HashMap<Uuid, Sender<Notification>>>
}

impl BroadcastChannel {

    pub async fn init() {
        BROADCAST_INSTANCE.get_or_init(|| async {
            let channel = Arc::new(BroadcastChannel::new());
            channel.clone().start_cleanup_task();
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

    fn new() -> Self {
        BroadcastChannel {
            channel: RwLock::new(HashMap::new()),
        }
    }

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
                    info!("Successfully sent {:?}", sc);
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
                        info!("Successfully sent {:?}", sc);
                    }
                    Err(err) => {
                        error!("Unable to broadcast notification: {}", err);
                    }
                }
            }
        }
    }

    pub async fn unsubscribe(&self, user_id: Uuid) {
        let mut lock = self.channel.write().await;
        lock.remove(&user_id);
    }
}
