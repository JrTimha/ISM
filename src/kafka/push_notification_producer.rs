use async_trait::async_trait;
use tracing::info;
use uuid::Uuid;
use crate::broadcast::Notification;
use crate::core::KafkaConfig;
use crate::errors::AppError;
use crate::kafka::event_producer::{KafkaEventProducer, LogEventProducer};
use crate::kafka::EventProducer;

pub enum PushNotificationProducer {
    Kafka(KafkaEventProducer),
    Logger(LogEventProducer)
}

#[async_trait]
impl EventProducer for PushNotificationProducer {
    async fn send_notification(&self, notification: Notification, to_user: Vec<Uuid>) -> Result<(), AppError> {
        match self {
            PushNotificationProducer::Kafka(producer) => producer.send_notification(notification, to_user).await,
            PushNotificationProducer::Logger(producer) => producer.send_notification(notification, to_user).await,
        }
    }
}


impl PushNotificationProducer {
    pub fn new(use_kafka: bool, kafka_config: KafkaConfig) -> Self {
        if use_kafka {
            info!("Kafka-Producer initializing.");
            PushNotificationProducer::Kafka(KafkaEventProducer::new(kafka_config))
        } else {
            PushNotificationProducer::Logger(LogEventProducer::new())
        }
    }
}