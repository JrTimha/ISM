use std::str::FromStr;
use std::sync::Arc;
use chrono::Utc;
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification};
use crate::broadcast::NotificationEvent::ChatMessage;
use crate::core::AppState;
use crate::errors::{AppError};
use crate::messaging::model::{Message, MessageBody, MessageDTO, MsgType, NewMessage, NewMessageBody, NewReplyBody, RepliedMessageDetails, ReplyBody};
use crate::model::LastMessagePreviewText;

pub struct MessageService;

impl MessageService {

    pub async fn send_message(
        state: Arc<AppState>,
        message: NewMessage,
        client_id: Uuid
    ) -> Result<MessageDTO, AppError>  {
        
        let users = state.room_repository.select_room_participants_ids(&message.chat_room_id).await?;
        if !users.contains(&client_id) {
            return Err(AppError::Blocked("User has not access to this room.".to_string()));
        };

        let msg_body = match message.msg_body.clone() {
            NewMessageBody::Text(text) => {
                MessageBody::Text(text)
            }
            NewMessageBody::Media(media) => {
                MessageBody::Media(media)
            }
            NewMessageBody::Reply(reply) => {
                let reply = MessageService::create_reply_message(&reply, &state, &message.chat_room_id).await.map_err(|err| {
                    AppError::ProcessingError(format!("Can't create reply message: {}", err.to_string()))
                })?;
                MessageBody::Reply(reply)
            }
        };

        let msg = Message::new(message.chat_room_id, client_id, msg_body).map_err(|_err| {
            AppError::ProcessingError("Can't create chat message.".to_string())
        })?;
        //save to nosql:
        state.message_repository.insert_data(msg.clone()).await?;


        let client_entity = state.room_repository.select_joined_user_by_id(&message.chat_room_id, &client_id).await?;

        let room_preview_text = MessageService::generate_room_preview_text(&message, client_entity.display_name);
        let preview_str = serde_json::to_string(&room_preview_text).map_err(|err| {
            AppError::ProcessingError(format!("Can't serialize message: {}", err.to_string()))
        })?;


        let mut tx = state.room_repository.start_transaction().await?;
        state.room_repository.update_last_room_message(&mut *tx, &message.chat_room_id, &preview_str).await?;
        state.room_repository.update_user_read_status(&mut *tx, &message.chat_room_id, &msg.sender_id).await?;
        tx.commit().await?;
        
        
        let mapped_msg = msg.to_dto().map_err(|err| {
            AppError::ProcessingError(format!("Can't serialize message: {}", err.to_string()))
        })?;

        BroadcastChannel::get().send_event_to_all(
            users,
            Notification {
                body: ChatMessage {message: mapped_msg.clone(), room_preview_text },
                created_at: Utc::now()
            }
        ).await;
        Ok(mapped_msg)
    }

    async fn create_reply_message(msg: &NewReplyBody, state: &Arc<AppState>, room_id: &Uuid) -> Result<ReplyBody, Box<dyn std::error::Error>> {
        let replied_to = state.message_repository.fetch_specific_message(&msg.reply_msg_id, room_id, &msg.reply_created_at).await?;

        let replied_body: MessageBody = serde_json::from_str(&replied_to.msg_body)?;

        let details = match replied_body {
            MessageBody::Text(text) => {
                RepliedMessageDetails::Text(text)
            }
            MessageBody::Media(media) => {
                RepliedMessageDetails::Media(media)
            }
            MessageBody::Reply(reply) => {
                RepliedMessageDetails::Reply {reply_text: reply.reply_text}
            }
            _ => {
                return Err(Box::from("Unknown Reply body"))
            }
        };

        let new_body = ReplyBody {
            reply_msg_id: replied_to.message_id,
            reply_sender_id: replied_to.sender_id,
            reply_msg_type: MsgType::from_str(&replied_to.msg_type)?,
            reply_created_at: replied_to.created_at,
            reply_msg_details: details,
            reply_text: msg.reply_text.clone(),
        };
        Ok(new_body)
    }

    fn generate_room_preview_text(msg: &NewMessage, username: String) -> LastMessagePreviewText {
        match &msg.msg_body {
            NewMessageBody::Text(body) => {
                LastMessagePreviewText::Text { sender_username: username, text: body.text.clone()}
            }
            NewMessageBody::Media(body) => {
                LastMessagePreviewText::Media { sender_username: username, media_type: body.media_type.clone()}
            }
            NewMessageBody::Reply(body) => {
                LastMessagePreviewText::Reply { sender_username: username, reply_text: body.reply_text.clone()}
            }
        }
    }



}