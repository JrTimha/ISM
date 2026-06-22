use crate::core::AppState;
use crate::core::errors::AppResponse;
use crate::messaging::model::{MessageBody, MessageDto, TimelinePage};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

pub struct TimelineService;

impl TimelineService {
    pub async fn scroll_chat_timeline(
        state: Arc<AppState>,
        room_id: Uuid,
        timestamp: DateTime<Utc>,
    ) -> AppResponse<TimelinePage> {
        let entities = state
            .chat_repository
            .fetch_messages(room_id, timestamp)
            .await?;

        // Collect the distinct authors of this page so the client can render every
        // message without a separate lookup — including authors that have since left.
        // Reply messages reference the original author (`reply_sender_id`), who may be
        // outside this page, so include them too.
        let mut sender_ids: Vec<Uuid> = Vec::with_capacity(entities.len());
        for message in &entities {
            sender_ids.push(message.sender_id);
            if let MessageBody::Reply(reply) = &message.msg_body.0 {
                sender_ids.push(reply.reply_sender_id);
            }
        }
        sender_ids.sort();
        sender_ids.dedup();

        let senders = state
            .room_repository
            .select_message_senders(&room_id, &sender_ids)
            .await?;
        let messages = entities.into_iter().map(MessageDto::from).collect();

        Ok(TimelinePage { messages, senders })
    }
}
