use crate::core::{Database, Repository};
use crate::messaging::model::{MessageBody, MessageEntity, MsgType};
use chrono::{DateTime, Utc};
use sqlx::{Error, Postgres};
use uuid::Uuid;

/// Chat message persistence.
#[derive(Clone)]
pub struct ChatRepository {
    db: Database,
}

impl Repository for ChatRepository {
    fn new(db: &Database) -> Self {
        Self { db: db.clone() }
    }
}

impl ChatRepository {
    pub async fn insert_message<'e, E>(&self, exec: E, message: &MessageEntity) -> Result<(), Error>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        sqlx::query!(
            r#"
            INSERT INTO chat_message (message_id, chat_room_id, sender_id, msg_body, msg_type, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            message.message_id,
            message.chat_room_id,
            message.sender_id,
            message.msg_body.clone() as sqlx::types::Json<MessageBody>,
            message.msg_type.clone() as MsgType,
            message.created_at
        )
        .execute(exec)
        .await?;
        Ok(())
    }

    pub async fn fetch_messages(&self, room_id: Uuid, before: DateTime<Utc>) -> Result<Vec<MessageEntity>, Error> {
        let messages = sqlx::query_as!(
            MessageEntity,
            r#"
            SELECT
                message_id,
                chat_room_id,
                sender_id,
                msg_body AS "msg_body: sqlx::types::Json<MessageBody>",
                msg_type AS "msg_type: MsgType",
                created_at
            FROM chat_message
            WHERE chat_room_id = $1 AND created_at < $2
            ORDER BY created_at DESC
            LIMIT 25
            "#,
            room_id,
            before
        )
        .fetch_all(self.db.pool())
        .await?;
        Ok(messages)
    }

    pub async fn fetch_message_by_id(&self, message_id: &Uuid, room_id: &Uuid) -> Result<MessageEntity, Error> {
        let message = sqlx::query_as!(
            MessageEntity,
            r#"
            SELECT
                message_id,
                chat_room_id,
                sender_id,
                msg_body AS "msg_body: sqlx::types::Json<MessageBody>",
                msg_type AS "msg_type: MsgType",
                created_at
            FROM chat_message
            WHERE message_id = $1 AND chat_room_id = $2
            "#,
            message_id,
            room_id
        )
        .fetch_one(self.db.pool())
        .await?;
        Ok(message)
    }

    pub async fn delete_room_messages<'e, E>(&self, exec: E, room_id: &Uuid) -> Result<(), Error>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        sqlx::query!("DELETE FROM chat_message WHERE chat_room_id = $1", room_id).execute(exec).await?;
        Ok(())
    }
}
