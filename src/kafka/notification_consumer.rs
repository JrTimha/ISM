use std::time::Duration;
use samsa::prelude::{BrokerAddress, ConsumerGroupBuilder, TcpConnection, TopicPartitionsBuilder};
use tokio_stream::StreamExt;
use log::{error, info};
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

    // Stream will do nothing unless consumed.
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(msg) => {
                msg.for_each(|notification| {
                    match String::from_utf8(notification.value.to_vec()) {
                        Ok(str) => info!("{}", str),
                        Err(err) => error!("{}", err),
                    }
                    info!("Received message: {}", notification.offset);
                });
            },
            Err(e) => {
                error!("Error: {e}");
            }
        }
    }
}