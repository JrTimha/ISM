use crate::broadcast::NotificationEvent::ChatMessage;
use crate::broadcast::{BroadcastChannel, Notification};
use crate::core::AppState;
use crate::core::errors::AppError;
use crate::messaging::model::{
    MessageBody, MessageDto, MessageEntity, NewMessage, NewMessageBody, NewReplyBody,
    RepliedMessageDetails, ReplyBody,
};
use crate::rooms::room::LastMessagePreviewText;
use crate::rooms::room_member::RoomContext;
use std::sync::Arc;
use uuid::Uuid;

pub struct MessageService;

impl MessageService {
    pub async fn send_message(
        state: Arc<AppState>,
        message: NewMessage,
        client_id: Uuid,
    ) -> Result<MessageDto, AppError> {
        // 1. Load room context from cache, fall back to DB
        let context = match state.cache.get_room_context(&message.chat_room_id).await? {
            Some(ctx) => ctx,
            None => {
                let members = state
                    .room_repository
                    .select_all_room_member(&message.chat_room_id)
                    .await?;
                let ctx = RoomContext { members };
                state
                    .cache
                    .set_room_context(&message.chat_room_id, &ctx)
                    .await?;
                ctx
            }
        };

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
                let reply =
                    MessageService::create_reply_message(&reply, &state, &message.chat_room_id)
                        .await
                        .map_err(|err| {
                            AppError::Processing(format!("Can't create reply message: {}", err))
                        })?;
                MessageBody::Reply(reply)
            }
        };

        let entity = MessageEntity::new(message.chat_room_id, client_id, msg_body);

        // 4. Generate preview text — display name from context, no DB call
        let room_preview_text =
            MessageService::generate_room_preview_text(&message, sender_display_name);

        // 5. Single atomic transaction: insert message + update room state in one CTE round-trip
        let mut tx = state.room_repository.start_transaction().await?;
        state
            .chat_repository
            .insert_message(&mut *tx, &entity)
            .await?;
        state
            .room_repository
            .apply_message_to_room(
                &mut *tx,
                &message.chat_room_id,
                &room_preview_text,
                &entity.sender_id,
                entity.created_at,
            )
            .await?;
        tx.commit().await?;

        // 6. Broadcast to all room members
        let dto = MessageDto::from(entity);
        let notification = Notification::new(ChatMessage {
            message: dto.clone(),
            room_preview_text,
            sender: sender_member,
        });
        BroadcastChannel::get()
            .send_event_to_all(context.member_ids(), notification)
            .await;
        Ok(dto)
    }

    async fn create_reply_message(
        msg: &NewReplyBody,
        state: &Arc<AppState>,
        room_id: &Uuid,
    ) -> Result<ReplyBody, Box<dyn std::error::Error>> {
        let replied_to = state
            .chat_repository
            .fetch_message_by_id(&msg.reply_msg_id, room_id)
            .await?;

        let details = match replied_to.msg_body.0 {
            MessageBody::Text(text) => RepliedMessageDetails::Text(text),
            MessageBody::Media(media) => RepliedMessageDetails::Media(media),
            MessageBody::Reply(reply) => RepliedMessageDetails::Reply {
                reply_text: reply.reply_text,
            },
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
}
