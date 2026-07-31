use crate::broadcast::NotificationEvent::ChatMessage;
use crate::core::errors::AppError;
use crate::core::{Database, Service};
use crate::messaging::ChatRepository;
use crate::messaging::model::{MessageBody, MessageDto, MessageEntity, NewMessage, NewMessageBody, NewReplyBody, RepliedMessageDetails, ReplyBody};
use crate::rooms::room::LastMessagePreviewText;
use crate::rooms::{RoomNotifier, RoomRepository};
use uuid::Uuid;

/// Sending chat messages.
#[derive(Clone)]
pub struct MessageService {
    /// Present because the message insert and the room's preview-text update must be one
    /// transaction across two repositories.
    db: Database,
    rooms: RoomRepository,
    chats: ChatRepository,
    notifier: RoomNotifier,
}

impl Service for MessageService {
    const NAME: &'static str = "MessageService";
}

impl MessageService {
    pub fn new(db: Database, rooms: RoomRepository, chats: ChatRepository, notifier: RoomNotifier) -> Self {
        Self { db, rooms, chats, notifier }
    }

    pub async fn send_message(&self, message: NewMessage, client_id: Uuid) -> Result<MessageDto, AppError> {
        // 1. Room membership, cached — this one lookup answers "may they post here?" and
        //    "what is their display name?" without a second query.
        let context = self.notifier.room_context(&message.chat_room_id).await?;

        // 2. Auth check + sender display name — no extra DB call
        let sender = context
            .find_member(&client_id)
            .ok_or_else(|| AppError::Forbidden("User hasn't access to this room.".to_string()))?;
        let sender_display_name = sender.display_name.clone();
        let sender_member = sender.clone();

        // 3. Build message body
        let msg_body = match message.msg_body.clone() {
            NewMessageBody::Text(text) => MessageBody::Text(text),
            NewMessageBody::Media(media) => MessageBody::Media(media),
            NewMessageBody::Reply(reply) => {
                let reply = self
                    .create_reply_message(&reply, &message.chat_room_id)
                    .await
                    .map_err(|err| AppError::Processing(format!("Can't create reply message: {}", err)))?;
                MessageBody::Reply(reply)
            }
        };

        let entity = MessageEntity::new(message.chat_room_id, client_id, msg_body);

        // 4. Generate preview text — display name from context, no DB call
        let room_preview_text = generate_room_preview_text(&message, sender_display_name);

        // 5. Single atomic transaction: insert message + update room state in one CTE round-trip
        let mut tx = self.db.begin().await?;
        self.chats.insert_message(&mut *tx, &entity).await?;
        self.rooms
            .apply_message_to_room(&mut tx, &message.chat_room_id, &room_preview_text, &entity.sender_id, entity.created_at)
            .await?;
        tx.commit().await?;

        // 6. Broadcast to all room members
        let dto = MessageDto::from(entity);
        self.notifier
            .notify_users(
                context.member_ids(),
                ChatMessage {
                    message: dto.clone(),
                    room_preview_text,
                    sender: sender_member,
                },
            )
            .await;
        Ok(dto)
    }

    async fn create_reply_message(&self, msg: &NewReplyBody, room_id: &Uuid) -> Result<ReplyBody, Box<dyn std::error::Error>> {
        let replied_to = self.chats.fetch_message_by_id(&msg.reply_msg_id, room_id).await?;

        let details = match replied_to.msg_body.0 {
            MessageBody::Text(text) => RepliedMessageDetails::Text(text),
            MessageBody::Media(media) => RepliedMessageDetails::Media(media),
            MessageBody::Reply(reply) => RepliedMessageDetails::Reply { reply_text: reply.reply_text },
            _ => return Err(Box::from("Cannot reply to a room change event")),
        };

        Ok(ReplyBody {
            reply_msg_id: replied_to.message_id,
            reply_sender_id: replied_to.sender_id,
            reply_msg_type: replied_to.msg_type,
            reply_created_at: replied_to.created_at,
            reply_msg_details: details,
            reply_text: msg.reply_text.clone(),
        })
    }
}

fn generate_room_preview_text(msg: &NewMessage, username: String) -> LastMessagePreviewText {
    match &msg.msg_body {
        NewMessageBody::Text(body) => LastMessagePreviewText::Text {
            sender_username: username,
            text: body.text.clone(),
        },
        NewMessageBody::Media(body) => LastMessagePreviewText::Media {
            sender_username: username,
            media_type: body.media_type.clone(),
        },
        NewMessageBody::Reply(body) => LastMessagePreviewText::Reply {
            sender_username: username,
            reply_text: body.reply_text.clone(),
        },
    }
}
