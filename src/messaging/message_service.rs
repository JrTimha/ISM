use std::sync::Arc;
use chrono::Utc;
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification};
use crate::broadcast::NotificationEvent::ChatMessage;
use crate::core::AppState;
use crate::errors::AppError;
use crate::messaging::model::{MessageBody, MessageDto, MessageEntity, NewMessage, NewMessageBody, NewReplyBody, RepliedMessageDetails, ReplyBody};
use crate::model::LastMessagePreviewText;

pub struct MessageService;

impl MessageService {

    pub async fn send_message(
        state: Arc<AppState>,
        message: NewMessage,
        client_id: Uuid,
    ) -> Result<MessageDto, AppError> {

        let mut users = state.cache.get_user_for_room(&message.chat_room_id).await?;

        if users.is_empty() {
            users = state.room_repository.select_room_participants_ids(&message.chat_room_id).await?;
            state.cache.set_user_for_room(&message.chat_room_id, &users).await?;
        }

        if !users.contains(&client_id) {
            return Err(AppError::Forbidden("User hasn't access to this room.".to_string()));
        }

        let msg_body = match message.msg_body.clone() {
            NewMessageBody::Text(text) => MessageBody::Text(text),
            NewMessageBody::Media(media) => MessageBody::Media(media),
            NewMessageBody::Reply(reply) => {
                let reply = MessageService::create_reply_message(&reply, &state, &message.chat_room_id).await
                    .map_err(|err| AppError::Processing(format!("Can't create reply message: {}", err)))?;
                MessageBody::Reply(reply)
            }
        };

        let entity = MessageEntity::new(message.chat_room_id, client_id, msg_body);

        //1. save message to postgresql:
        state.chat_repository.insert_message(&entity).await?;

        //2. generate new room preview text and save it to sql db:
        let client_entity = state.room_repository.select_joined_user_by_id(&message.chat_room_id, &client_id).await?;
        let room_preview_text = MessageService::generate_room_preview_text(&message, client_entity.display_name);
        let preview_str = serde_json::to_string(&room_preview_text)?;

        let mut tx = state.room_repository.start_transaction().await?;
        state.room_repository.update_last_room_message(&mut *tx, &message.chat_room_id, &preview_str).await?;
        state.room_repository.update_user_read_status(&mut *tx, &message.chat_room_id, &entity.sender_id).await?;
        tx.commit().await?;

        //3. broadcast message to all room members:
        let dto = MessageDto::from(entity);
        let notification = Notification {
            body: ChatMessage { message: dto.clone(), room_preview_text },
            created_at: Utc::now(),
        };
        BroadcastChannel::get().send_event_to_all(users, notification).await;
        Ok(dto)
    }

    async fn create_reply_message(msg: &NewReplyBody, state: &Arc<AppState>, room_id: &Uuid) -> Result<ReplyBody, Box<dyn std::error::Error>> {
        let replied_to = state.chat_repository.fetch_message_by_id(&msg.reply_msg_id, room_id).await?;

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

    fn generate_room_preview_text(msg: &NewMessage, username: String) -> LastMessagePreviewText {
        match &msg.msg_body {
            NewMessageBody::Text(body) => LastMessagePreviewText::Text { sender_username: username, text: body.text.clone() },
            NewMessageBody::Media(body) => LastMessagePreviewText::Media { sender_username: username, media_type: body.media_type.clone() },
            NewMessageBody::Reply(body) => LastMessagePreviewText::Reply { sender_username: username, reply_text: body.reply_text.clone() },
        }
    }
}