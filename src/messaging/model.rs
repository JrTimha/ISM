use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;
use crate::broadcast::Notification;
use crate::broadcast::NotificationEvent::ChatMessage;
use crate::errors::AppError;
use crate::model::{LastMessagePreviewText, RoomMember};

#[derive(sqlx::Type, Debug, Deserialize, Serialize, Clone, PartialEq)]
#[sqlx(type_name = "msg_type")]
pub enum MsgType {
    Text,
    Media,
    RoomChange,
    Reply,
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct Message {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: sqlx::types::Json<MessageBody>,
    pub msg_type: MsgType,
    pub created_at: DateTime<Utc>
}

impl Message {

    pub fn new(room_id: Uuid, sender_id: Uuid, msg_body: MessageBody) -> Message {
        let msg_type = match &msg_body {
            MessageBody::Text(_) => MsgType::Text,
            MessageBody::Media(_) => MsgType::Media,
            MessageBody::Reply(_) => MsgType::Reply,
            MessageBody::RoomChange(_) => MsgType::RoomChange,
        };
        Message {
            chat_room_id: room_id,
            message_id: Uuid::new_v4(),
            sender_id,
            msg_body: sqlx::types::Json(msg_body),
            msg_type,
            created_at: Utc::now(),
        }
    }

    pub fn to_dto(&self) -> MessageDTO {
        MessageDTO {
            chat_room_id: self.chat_room_id,
            message_id: self.message_id,
            sender_id: self.sender_id,
            msg_body: self.msg_body.0.clone(),
            msg_type: self.msg_type.clone(),
            created_at: self.created_at,
        }
    }

    pub fn to_notification(&self, preview_text: LastMessagePreviewText) -> Notification {
        Notification {
            body: ChatMessage { message: self.to_dto(), room_preview_text: preview_text },
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageDTO {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: MessageBody,
    pub msg_type: MsgType,
    pub created_at: DateTime<Utc>
}

impl MessageDTO {

    pub fn from_json_str(s: &str) -> Result<MessageDTO, AppError> {
        serde_json::from_str(s).map_err(|err| {
            AppError::Processing(format!("Error parsing message: {}", err))
        })
    }

    pub fn json_str(&self) -> Result<String, AppError> {
        serde_json::to_string(self).map_err(|err| {
            AppError::Processing(format!("Error parsing message: {}", err))
        })
    }
}


#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum MessageBody {
    Text(TextBody),
    Media(MediaBody),
    Reply(ReplyBody),
    RoomChange(RoomChangeBody)
}

#[derive(Deserialize, Serialize, Debug, Clone, Validate)]
#[serde(rename_all = "camelCase")]
pub struct TextBody {
    #[validate(length(min = 1, max = 4000, message = "must be between 1 and 4000 characters long."))]
    pub text: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, Validate)]
#[serde(rename_all = "camelCase")]
pub struct MediaBody {
    #[validate(length(min = 1, max = 250, message = "must be between 1 and 250 characters long."))]
    pub media_url: String,
    #[validate(length(min = 1, max = 80, message = "must be between 1 and 80 characters long."))]
    pub media_type: String,
    pub mime_type: Option<String>,
    pub alt_text: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReplyBody {
    pub reply_msg_id: Uuid,
    pub reply_sender_id: Uuid,
    pub reply_msg_type: MsgType,
    pub reply_created_at: DateTime<Utc>,
    pub reply_msg_details: RepliedMessageDetails,
    pub reply_text: String
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum RepliedMessageDetails {
    Text(TextBody),
    Media(MediaBody),
    Reply {reply_text: String}
}


#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum RoomChangeBody {
    UserJoined {related_user: RoomMember },
    UserLeft {related_user: RoomMember },
    UserInvited {related_user: RoomMember }
}


#[derive(Deserialize, Debug, Clone, Validate)]
#[serde(rename_all = "camelCase")]
pub struct NewMessage {
    pub chat_room_id: Uuid,
    #[validate(nested)]
    pub msg_body: NewMessageBody,
    pub msg_type: MsgType
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum NewMessageBody {
    Text(TextBody),
    Media(MediaBody),
    Reply(NewReplyBody)
}

impl Validate for NewMessageBody {
    fn validate(&self) -> Result<(), validator::ValidationErrors> {
        match self {
            NewMessageBody::Text(body) => body.validate(),
            NewMessageBody::Media(body) => body.validate(),
            NewMessageBody::Reply(body) => body.validate(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Validate)]
#[serde(rename_all = "camelCase")]
pub struct NewReplyBody {
    pub reply_msg_id: Uuid,
    #[validate(length(min = 1, max = 4000, message = "must be between 1 and 4000 characters long."))]
    pub reply_text: String
}