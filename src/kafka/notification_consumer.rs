use std::sync::Arc;
use std::time::Duration;
use samsa::prelude::{BrokerAddress, ConsumeMessage, ConsumerGroupBuilder, TcpConnection, TopicPartitionsBuilder};
use log::{error, info};
use tokio_stream::StreamExt;
use tracing_subscriber::filter::combinator::Not;
use crate::broadcast::{get_broadcast_channel, BroadcastChannel, NewNotification, Notification, NotificationEvent};
use crate::core::KafkaConfig;


pub async fn start_consumer(config: KafkaConfig) {
    let bootstrap_address = vec![BrokerAddress {
        host: config.bootstrap_host,
        port: config.bootstrap_port
    }];

    let partitions = config.partition;
    let topic_name = config.topic;
    let assignment = TopicPartitionsBuilder::new()
        .assign(topic_name, partitions)
        .build();

    let consumer = ConsumerGroupBuilder::<TcpConnection>::new(
        bootstrap_address,
        config.consumer_group,
        assignment,
    ).await
        .expect("Could not create consumer.")
        .client_id(config.client_id)
        .build()
        .await
        .expect("Could not create consumer.");

    let stream = consumer.into_stream().throttle(Duration::from_secs(5));
    // have to pin streams before iterating
    tokio::pin!(stream);
    let broadcast = get_broadcast_channel().await;

    // Stream will do nothing unless consumed.
    while let Some(message_stream) = stream.next().await {
        match message_stream {
            Ok(messages) => {
                for entry in messages {
                    process_message_entry(entry, &broadcast).await;
                }
            },
            Err(e) => {
                error!("Error: {e}");
            }
        }
    }
}

async fn process_message_entry(entry: ConsumeMessage, broadcast: &Arc<BroadcastChannel>) {
    match serde_json::from_slice::<NewNotification>(&entry.value.to_vec()) {
        Ok(value) => {
            let notification = Notification {
                notification_event: value.event_type,
                body: value.body,
                created_at: value.created_at,
                display_value: None
            };
            broadcast.send_event(notification, &value.to_user).await;
        },
        Err(err) => {
            error!("Deserialization failed: {err}");
        }
    }
}