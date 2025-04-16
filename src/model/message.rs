use std::error::Error;
use std::fmt;
use std::str::FromStr;
use chrono::{DateTime, Utc};
use scylla::{DeserializeRow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum MsgType {
    Text,
    Media,
    System,
    Reply,
}

#[derive(DeserializeRow, Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: String,
    pub msg_type: String,
    pub created_at: DateTime<Utc>
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageDTO {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: MessageBody,
    pub msg_type: MsgType,
    pub created_at: DateTime<Utc>
}


#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum MessageBody {
    Text(TextBody),
    Media(MediaBody),
    Reply(ReplyBody),
    System(SystemBody)
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextBody {
    pub text: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MediaBody {
    pub media_url: String,
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
#[serde(rename_all = "camelCase")]
pub struct SystemBody {
    pub message: String,
    pub system_msg_type: String,
    pub linked_to_user: Option<Uuid>
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NewMessage {
    pub chat_room_id: Uuid,
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

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NewReplyBody {
    pub reply_msg_id: Uuid,
    pub reply_created_at: DateTime<Utc>,
    pub reply_text: String
}


impl fmt::Display for MsgType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MsgType::Text => write!(f, "Text"),
            MsgType::Media => write!(f, "Media"),
            MsgType::System => write!(f, "System"),
            MsgType::Reply => write!(f, "Reply")
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseMessageTypeError;

impl fmt::Display for ParseMessageTypeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Ungültiger MessageType-String")
    }
}
impl Error for ParseMessageTypeError {}


impl FromStr for MsgType {
    type Err = ParseMessageTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Text" => Ok(MsgType::Text),
            "Media" => Ok(MsgType::Media),
            "System" => Ok(MsgType::System),
            "Reply" => Ok(MsgType::Reply),
            _ => Err(ParseMessageTypeError),
        }
    }
}