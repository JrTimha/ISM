use std::error::Error;
use std::fmt;
use std::str::FromStr;
use chrono::{DateTime, Utc};
use scylla::{DeserializeRow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::model::RoomMember;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum MsgType {
    Text,
    Media,
    RoomChange,
    Reply,
}

#[derive(DeserializeRow, Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    //it is a JSON string in scyllaDb, because the rust client can't handle JSON to struct at the moment
    pub msg_body: String,
    //the rust client from scylla can't handle enums at the moment, so we have to use a string and map it to the enum later
    pub msg_type: String,
    pub created_at: DateTime<Utc>
}

impl Message {
    
    pub fn new(room_id: Uuid, sender_id: Uuid, msg_body: MessageBody) -> Result<Message, serde_json::Error> {
        let typ = match msg_body {
            MessageBody::Text(_) => MsgType::Text,
            MessageBody::Media(_) => MsgType::Media,
            MessageBody::Reply(_) => MsgType::Reply,
            MessageBody::RoomChange(_) => MsgType::RoomChange
        };
        let body_json = serde_json::to_string(&msg_body)?;
        let msg = Message {
            chat_room_id: room_id,
            message_id: Uuid::new_v4(),
            sender_id: sender_id,
            msg_body: body_json,
            msg_type: typ.to_string(),
            created_at: Utc::now()
        };
        Ok(msg)
    }
    
    pub fn to_dto(&self) -> Result<MessageDTO, Box<dyn std::error::Error>> {
        let message = MessageDTO {
            chat_room_id: self.chat_room_id,
            message_id: self.message_id,
            sender_id: self.sender_id,
            msg_body: serde_json::from_str(&self.msg_body)?,
            msg_type: self.msg_type.parse()?,
            created_at: self.created_at
        };
        Ok(message)
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


#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum MessageBody {
    /**
    * This is the most common message type, just a text message.
    */
    Text(TextBody),
    /**
    * For linking urls to images, videos or other media.
    */
    Media(MediaBody),
    /**
    * Replying to a message, alle message types supported.
    */
    Reply(ReplyBody),
    /**
    * For room events like user joining or leaving.
    */
    RoomChange(RoomChangeBody)
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
#[serde(tag = "type")]
pub enum RoomChangeBody {
    UserJoined {related_user: RoomMember },
    UserLeft {related_user: RoomMember },
    UserInvited {related_user: RoomMember }
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
            MsgType::RoomChange => write!(f, "RoomChange"),
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
            "RoomChange" => Ok(MsgType::RoomChange),
            "Reply" => Ok(MsgType::Reply),
            _ => Err(ParseMessageTypeError),
        }
    }
}