use std::sync::Arc;
use chrono::{DateTime, Utc};
use scylla::{QueryResult, Session, SessionBuilder};
use scylla::transport::ClusterData;
use scylla::transport::errors::{NewSessionError, QueryError};
use crate::core::{MessageDbConfig};
use tokio::sync::OnceCell;
use futures::TryStreamExt;
use log::{debug, error, info};
use uuid::Uuid;
use crate::model::Message;

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

pub async fn create_keyspace_with_tables(session: &Session) {
    let queries = [
        "CREATE KEYSPACE IF NOT EXISTS messaging WITH REPLICATION = {'class' : 'NetworkTopologyStrategy', 'replication_factor' : 1}",
        "CREATE TABLE IF NOT EXISTS messaging.chat_messages (
            chat_room_id UUID,
            message_id UUID,
            sender_id UUID,
            msg_body TEXT,
            msg_type TEXT,
            created_at TIMESTAMP,
            PRIMARY KEY ((chat_room_id), created_at, message_id)
        )"
    ];
    for query in queries.iter() {
        if let Err(e) = session.query_unpaged(*query, &[]).await {
            error!("Error executing query '{}': {:?}", query, e);
        } else {
            debug!("Successfully executed query: '{}'", query);
        }
    }
}

impl MessageRepository {

    async fn new(config: &MessageDbConfig) -> Result<Self, NewSessionError> {
        let session = SessionBuilder::new()
            .known_node(&config.db_url)
            .user(&config.db_user, &config.db_password)
            .build()
            .await?;
        if config.with_db_init {
            create_keyspace_with_tables(&session).await;
        }
        if let Err(err) = session.use_keyspace(&config.db_keyspace, true).await {
            error!("Failed to use keyspace {:?}", err);
            std::process::exit(1);
        }
        Ok(MessageRepository { session: Arc::new(session) })
    }

    pub async fn fetch_data(&self, timestamp: DateTime<Utc>, room_id: Uuid) -> Result<Vec<Message>,  Box<dyn std::error::Error>> {
        let session = self.session.clone();
        let mut iter = session.query_iter("SELECT chat_room_id, message_id, sender_id, msg_body, created_at, msg_type FROM chat_messages WHERE chat_room_id = ? AND created_at < ? ORDER BY created_at DESC LIMIT 25", (room_id, timestamp))
            .await?.rows_stream::<Message>()?;
        let mut messages: Vec<Message> = Vec::new();
        while let Some(next) = iter.try_next().await? { messages.push(next) }
        Ok(messages)
    }

    pub async fn insert_data(&self, message: Message) -> Result<QueryResult, QueryError> {
       let session = self.session.clone();
       session.query_unpaged(
            "INSERT INTO chat_messages (chat_room_id, message_id, sender_id, msg_body, msg_type, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            (message.chat_room_id, message.message_id, message.sender_id, message.msg_body, message.msg_type, message.created_at)
       ).await
    }

    pub async fn test_connection(&self) -> Result<Arc<ClusterData>, scylla::transport::errors::RequestError> {
        let session = self.session.clone();
        let data = session.get_cluster_data();
        Ok(data)
    }

}




