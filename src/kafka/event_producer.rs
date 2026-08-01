use crate::broadcast::Notification;
use crate::core::errors::AppError;
use crate::core::{KafkaConfig, StartupError, StartupResult};
use crate::kafka::model::PushNotification;
use async_trait::async_trait;
use rdkafka::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::time::Duration;
use tracing::{debug, error};
use uuid::Uuid;

#[async_trait]
pub trait EventProducer: Send + Sync {
    async fn send_notification(&self, notification: Notification, to_user: Vec<Uuid>) -> Result<(), AppError>;
}

pub struct KafkaEventProducer {
    producer: FutureProducer,
    config: KafkaConfig,
}

impl KafkaEventProducer {
    pub fn connect(config: KafkaConfig) -> StartupResult<Self> {
        let server = format!("{}:{}", config.bootstrap_host, config.bootstrap_port);
        let producer = ClientConfig::new()
            .set("bootstrap.servers", &server)
            .set("enable.idempotence", "true")
            .create()
            .map_err(|error| StartupError::Kafka(error.to_string()))?;
        Ok(Self { producer, config })
    }
}

#[async_trait]
impl EventProducer for KafkaEventProducer {
    async fn send_notification(&self, notification: Notification, to_user: Vec<Uuid>) -> Result<(), AppError> {
        let payload = serde_json::to_string(&PushNotification { to_user, notification })?;

        let response = self
            .producer
            .send(
                FutureRecord::<(), String>::to(&self.config.topic).payload(&payload).headers(generate_header()),
                Duration::from_secs(0),
            )
            .await;
        match response {
            Ok(delivery) => {
                debug!("Delivery result: {:?}", delivery);
                Ok(())
            }
            Err((kafka_error, _)) => {
                error!("Kafka event delivery failed: {:?}", kafka_error.to_string());
                Err(AppError::Processing("Unable to send push notification".to_string()))
            }
        }
    }
}

pub struct LogEventProducer;

impl LogEventProducer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl EventProducer for LogEventProducer {
    async fn send_notification(&self, _notification: Notification, _to_user: Vec<Uuid>) -> Result<(), AppError> {
        Ok(())
    }
}

/// Records what would have been pushed, so a test can assert on the batching.
///
/// Test-only, and it earns the `#[cfg(test)]` variant it adds to [`PushNotificationProducer`]:
/// "one record for the offline set instead of one per user" is the point of the fan-out change,
/// and neither the Kafka producer nor [`LogEventProducer`] lets anyone see it happen.
///
/// [`PushNotificationProducer`]: crate::kafka::PushNotificationProducer
#[cfg(test)]
#[derive(Default)]
pub struct RecordingEventProducer {
    sent: std::sync::Mutex<Vec<(Notification, Vec<Uuid>)>>,
}

#[cfg(test)]
impl RecordingEventProducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `send_notification` call so far, in order.
    pub fn sent(&self) -> Vec<(Notification, Vec<Uuid>)> {
        self.sent.lock().expect("recorder mutex").clone()
    }
}

#[cfg(test)]
#[async_trait]
impl EventProducer for RecordingEventProducer {
    async fn send_notification(&self, notification: Notification, to_user: Vec<Uuid>) -> Result<(), AppError> {
        self.sent.lock().expect("recorder mutex").push((notification, to_user));
        Ok(())
    }
}

fn generate_header() -> OwnedHeaders {
    OwnedHeaders::new()
        .insert(Header {
            key: "__TypeId__",
            value: Some("com.meventure.api.notifications.model.UndeliveredMessage".as_bytes()),
        })
        .insert(Header {
            key: "contentType",
            value: Some("application/json".as_bytes()),
        })
}
