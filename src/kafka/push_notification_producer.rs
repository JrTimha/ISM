use crate::broadcast::Notification;
use crate::core::errors::AppError;
use crate::core::{KafkaConfig, StartupResult};
use crate::kafka::EventProducer;
use crate::kafka::event_producer::{KafkaEventProducer, LogEventProducer};
use async_trait::async_trait;
use tracing::info;
use uuid::Uuid;

pub enum PushNotificationProducer {
    Kafka(KafkaEventProducer),
    Logger(LogEventProducer),
    /// Test-only. Behind an `Arc` because the bus takes the producer by value, and a test has to
    /// keep a handle to read back what was sent.
    #[cfg(test)]
    Recording(std::sync::Arc<crate::kafka::event_producer::RecordingEventProducer>),
}

#[async_trait]
impl EventProducer for PushNotificationProducer {
    async fn send_notification(&self, notification: Notification, to_user: Vec<Uuid>) -> Result<(), AppError> {
        match self {
            PushNotificationProducer::Kafka(producer) => producer.send_notification(notification, to_user).await,
            PushNotificationProducer::Logger(producer) => producer.send_notification(notification, to_user).await,
            #[cfg(test)]
            PushNotificationProducer::Recording(producer) => producer.send_notification(notification, to_user).await,
        }
    }
}

impl PushNotificationProducer {
    /// Picks the push-notification backend from config.
    ///
    /// An enum rather than `Box<dyn EventProducer>`: the backends are known at compile time, so
    /// the enum dispatches statically and keeps the type concrete. The third variant is
    /// `#[cfg(test)]` and never reaches a release build.
    pub fn connect(use_kafka: bool, kafka_config: KafkaConfig) -> StartupResult<Self> {
        if use_kafka {
            info!("Kafka-Producer initializing.");
            Ok(PushNotificationProducer::Kafka(KafkaEventProducer::connect(kafka_config)?))
        } else {
            Ok(PushNotificationProducer::Logger(LogEventProducer::new()))
        }
    }
}
