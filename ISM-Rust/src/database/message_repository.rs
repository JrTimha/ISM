use std::sync::Arc;
use scylla::{Session, SessionBuilder};
use scylla::transport::ClusterData;
use scylla::transport::errors::NewSessionError;
use crate::core::{MessageDbConfig};
use tokio::sync::OnceCell;
use futures::TryStreamExt;
use log::{error, info};
use crate::database::message::Message;

static DB_INSTANCE: OnceCell<Arc<MessageRepository>> = OnceCell::const_new();

pub async fn init_message_db(config: &MessageDbConfig) {
    DB_INSTANCE
        .get_or_init(|| async {
        let db = match MessageRepository::new(config).await {
            Ok(db) => {
                info!("Initialized MessageRepository.");
                db
            }
            Err(err) => {
                error!("Failed to initialize MessageRepository: {:?}", err);
                std::process::exit(1);
            }
        };
        Arc::new(db)
    }).await;
}

pub async fn get_message_repository_instance() -> Arc<MessageRepository> {
    DB_INSTANCE.get().expect("Message-DB instance not initialized. Please call init_message_db() first!").clone()
}

pub struct MessageRepository {
    session: Arc<Session>,
}

impl MessageRepository {

    async fn new(config: &MessageDbConfig) -> Result<Self, NewSessionError> {
        let session = SessionBuilder::new()
            .known_node(&config.db_url)
            .use_keyspace(&config.db_keyspace, true)
            .user(&config.db_user, &config.db_password)
            .build()
            .await?;
        Ok(MessageRepository { session: Arc::new(session) })
    }

    pub async fn fetch_data(&self) -> Result<Vec<Message>,  Box<dyn std::error::Error>> {
        let session = self.session.clone();
        let mut iter = session.query_iter("SELECT message_id, sender_id, receiver_id, msg_body, created_at, msg_type, has_read FROM messages", &[])
            .await?.rows_stream::<Message>()?;
        let mut messages: Vec<Message> = Vec::new();
        while let Some(next) = iter.try_next().await? { messages.push(next) }
        Ok(messages)
    }

    pub async fn insert_data(&self) -> Result<String, scylla::transport::errors::QueryError> {
        Ok("Data inserted successfully".to_string())
    }

    pub async fn test_connection(&self) -> Result<Arc<ClusterData>, scylla::transport::errors::RequestError> {
        let session = self.session.clone();
        let data = session.get_cluster_data();
        Ok(data)
    }

}




