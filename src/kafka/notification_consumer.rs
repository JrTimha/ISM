use std::sync::Arc;
use std::time::Duration;
use samsa::prelude::{BrokerAddress, ConsumeMessage, ConsumerGroup, ConsumerGroupBuilder, TcpConnection, TopicPartitionsBuilder};
use log::{debug, error};
use tokio_stream::StreamExt;
use crate::broadcast::{BroadcastChannel, SendNotification};
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

    let consumer: ConsumerGroup<TcpConnection> = ConsumerGroupBuilder::<TcpConnection>::new(
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
    let broadcast = BroadcastChannel::get().clone();

    // have to pin streams before iterating
    tokio::pin!(stream);

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
    match serde_json::from_slice::<SendNotification>(&entry.value.to_vec()) {
        Ok(value) => {
            broadcast.send_event(value.body, &value.to_user).await;
            debug!("Sent event, offset: {}", entry.offset);
        },
        Err(err) => {
            error!("Deserialization failed: {err}");
        }
    }
}