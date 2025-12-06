use std::time::Duration;
use async_trait::async_trait;
use rdkafka::{ClientConfig};
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use tracing::{debug, error};
use uuid::Uuid;
use crate::broadcast::Notification;
use crate::core::KafkaConfig;
use crate::errors::AppError;
use crate::kafka::model::PushNotification;

#[async_trait]
pub trait EventProducer: Send + Sync {
    async fn send_notification(&self, notification: Notification, to_user: Vec<Uuid>) -> Result<(), AppError>;
}

pub struct KafkaEventProducer {
    producer: FutureProducer,
    config: KafkaConfig,
}

impl KafkaEventProducer {
    pub fn new(config: KafkaConfig) -> Self {
        let server = format!("{}:{}", config.bootstrap_host, config.bootstrap_port);
        let producer = ClientConfig::new()
            .set("bootstrap.servers", &server)
            .set("enable.idempotence", "true")
            .create()
            .expect("Producer creation failed");
        Self { producer, config }
    }
}

#[async_trait]
impl EventProducer for KafkaEventProducer {


    async fn send_notification(&self, notification: Notification, to_user: Vec<Uuid>) -> Result<(), AppError> {
        let payload = serde_json::to_string(&PushNotification{to_user, notification})
            .map_err(|e| AppError::from(e))?;
        let response = self.producer.send(
            FutureRecord::<(), String>::to(&self.config.topic)
                .payload(&payload)
                .headers(
                    OwnedHeaders::new()
                        .insert(Header {
                            key: "__TypeId__",
                            value: Some("com.meventure.api.notifications.model.UndeliveredMessage".as_bytes()),
                        })
                        .insert(Header {
                            key: "contentType",
                            value: Some("application/json".as_bytes()),
                        })
                ),
            Duration::from_secs(0),
        ).await;
        match response {
            Ok(delivery) => {
                debug!("Delivery result: {:?}", delivery);
                Ok(())
            }
            Err((kafka_error, _)) => {
                error!("Kafka event delivery failed: {:?}", kafka_error.to_string());
                Err(AppError::ProcessingError("Unable to send push notification".to_string()))
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