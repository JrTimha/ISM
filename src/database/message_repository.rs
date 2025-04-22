use std::sync::Arc;
use chrono::{DateTime, Utc};
use crate::core::{MessageDbConfig};
use futures::{TryStreamExt};
use log::{debug, error, info};
use scylla::client::pager::TypedRowStream;
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use scylla::errors::{ExecutionError, NewSessionError, UseKeyspaceError};
use scylla::response::query_result::QueryResult;
use uuid::Uuid;
use crate::model::Message;

#[derive(Debug, Clone)]
pub struct MessageRepository {
    session: Arc<Session>,
}

impl MessageRepository {

    pub async fn new(config: &MessageDbConfig) -> Result<Self, NewSessionError> {
        let session = match SessionBuilder::new()
            .known_node(&config.db_url)
            .user(&config.db_user, &config.db_password)
            .build()
            .await
        {
            Ok(session) =>  {
                info!("Connection to the message database established.");
                session
            },
            Err(err) => {
                error!("Failed to create session to the message database: {:?}", err);
                std::process::exit(1);
            }
        };
        let repository = MessageRepository { session: Arc::new(session) };
        if config.with_db_init {
            repository.create_keyspace_with_tables().await;
        }

        if let Err(err) = repository.change_keyspace(&config.db_keyspace).await {
            error!("Failed to use keyspace {:?}", err);
            std::process::exit(1);
        }
        Ok(repository)
    }

    pub async fn fetch_data(&self, timestamp: DateTime<Utc>, room_id: Uuid) -> Result<Vec<Message>,  Box<dyn std::error::Error>> {
        let session = self.session.clone();
        let mut iter: TypedRowStream<Message> = session.query_iter("SELECT chat_room_id, message_id, sender_id, msg_body, created_at, msg_type FROM chat_messages WHERE chat_room_id = ? AND created_at < ? ORDER BY created_at DESC LIMIT 25", (room_id, timestamp))
            .await?.rows_stream::<Message>()?;
        let mut messages: Vec<Message> = Vec::new();
        while let Some(next) = iter.try_next().await? { messages.push(next) }
        Ok(messages)
    }

    pub async fn fetch_specific_message(&self, message_id: &Uuid, room_id: &Uuid, created: &DateTime<Utc>) -> Result<Message, Box<dyn std::error::Error>> {
        let session = self.session.clone();
        let mut iter = session.query_iter("SELECT chat_room_id, message_id, sender_id, msg_body, created_at, msg_type FROM chat_messages WHERE chat_room_id = ? AND created_at = ? AND message_id = ?", (room_id, created, message_id))
            .await?.rows_stream::<Message>()?;
        match iter.try_next().await? {
            Some(message) => Ok(message),
            None => Err("Message not found".into())
        }
    }

    pub async fn insert_data(&self, message: Message) -> Result<QueryResult, ExecutionError> {
       let session = self.session.clone();
       session.query_unpaged(
            "INSERT INTO chat_messages (chat_room_id, message_id, sender_id, msg_body, msg_type, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            (message.chat_room_id, message.message_id, message.sender_id, message.msg_body, message.msg_type, message.created_at)
       ).await
    }

    async fn create_keyspace_with_tables(&self) {
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
            if let Err(e) = self.session.query_unpaged(*query, &[]).await {
                error!("Error executing query '{}': {:?}", query, e);
            } else {
                debug!("Successfully executed query: '{}'", query);
            }
        }
    }
    
    pub async fn clear_chat_room_messages(&self, room_id: &Uuid) -> Result<(), ExecutionError> {
        let session = self.session.clone();
        session.query_unpaged("DELETE FROM chat_messages WHERE chat_room_id = ?", (room_id,)).await?;
        Ok(())
    }

    async fn change_keyspace(&self, keyspace: &String) -> Result<(), UseKeyspaceError> {
        self.session.use_keyspace(keyspace, true).await?;
        Ok(())
    }

}




