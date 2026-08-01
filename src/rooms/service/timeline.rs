use crate::core::Service;
use crate::core::errors::{AppError, AppResponse};
use crate::messaging::ChatRepository;
use crate::messaging::entity::MessageBodyJson;
use crate::messaging::response::{MessageResponse, TimelinePageResponse};
use crate::rooms::RoomRepository;
use crate::rooms::response::RoomMemberResponse;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Reads a room's message history.
#[derive(Clone)]
pub struct TimelineService {
    rooms: RoomRepository,
    chats: ChatRepository,
}

impl Service for TimelineService {
    const NAME: &'static str = "TimelineService";
}

impl TimelineService {
    pub fn new(rooms: RoomRepository, chats: ChatRepository) -> Self {
        Self { rooms, chats }
    }

    /// One page of a room's timeline, newest first.
    ///
    /// Membership is checked here rather than in the handler: whether the caller may read this
    /// room is a question only the database can answer, which makes it service business.
    pub async fn scroll_chat_timeline(&self, client_id: Uuid, room_id: Uuid, timestamp: DateTime<Utc>) -> AppResponse<TimelinePageResponse> {
        if !self.rooms.is_user_in_room(&client_id, &room_id).await? {
            return Err(AppError::Forbidden("User is not a member of this room.".to_string()));
        }

        let entities = self.chats.fetch_messages(room_id, timestamp).await?;

        // Collect the distinct authors of this page so the client can render every
        // message without a separate lookup — including authors that have since left.
        // Reply messages reference the original author (`reply_sender_id`), who may be
        // outside this page, so include them too.
        let mut sender_ids: Vec<Uuid> = Vec::with_capacity(entities.len());
        for message in &entities {
            sender_ids.push(message.sender_id);
            if let MessageBodyJson::Reply(reply) = &message.msg_body.0 {
                sender_ids.push(reply.reply_sender_id);
            }
        }
        sender_ids.sort();
        sender_ids.dedup();

        let senders = self.rooms.select_message_senders(&room_id, &sender_ids).await?;
        let messages = entities.into_iter().map(MessageResponse::from).collect();

        Ok(TimelinePageResponse {
            messages,
            senders: senders.into_iter().map(RoomMemberResponse::from).collect(),
        })
    }
}
