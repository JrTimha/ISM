//! Client-facing shapes for the messaging domain.
//!
//! These mirror the `…Json` storage types in `entity.rs` field for field, which is what keeps the
//! payload byte-identical today. They are separate types so that stops being a constraint: renaming
//! a field here is an API change with a migration note, renaming it there makes existing rows
//! undecodable.

use crate::core::ApiResponse;
use crate::messaging::entity::{MediaJson, MessageBodyJson, MessageRow, RepliedMessageJson, ReplyJson, RoomChangeJson, TextJson};
use crate::messaging::model::MsgType;
use crate::rooms::entity::RoomMemberSnapshotJson;
use crate::rooms::response::RoomMemberResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A chat message.
///
/// `Deserialize` under the notification exception in [`ApiResponse`]: embedded in `ChatMessage`,
/// `RoomChangeEvent` and `NewRoom`, which round-trip through the Redis replay stream.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageResponse {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: MessageBodyResponse,
    pub msg_type: MsgType,
    pub created_at: DateTime<Utc>,
}

impl ApiResponse for MessageResponse {}

impl From<MessageRow> for MessageResponse {
    fn from(row: MessageRow) -> Self {
        MessageResponse {
            chat_room_id: row.chat_room_id,
            message_id: row.message_id,
            sender_id: row.sender_id,
            msg_body: MessageBodyResponse::from(row.msg_body.0),
            msg_type: row.msg_type,
            created_at: row.created_at,
        }
    }
}

/// The body of a message. `untagged`, matching the stored representation — clients discriminate on
/// the sibling `msgType` field.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum MessageBodyResponse {
    Text(TextBodyResponse),
    Media(MediaBodyResponse),
    Reply(ReplyBodyResponse),
    RoomChange(RoomChangeResponse),
}

impl ApiResponse for MessageBodyResponse {}

impl From<MessageBodyJson> for MessageBodyResponse {
    fn from(stored: MessageBodyJson) -> Self {
        match stored {
            MessageBodyJson::Text(body) => MessageBodyResponse::Text(body.into()),
            MessageBodyJson::Media(body) => MessageBodyResponse::Media(body.into()),
            MessageBodyJson::Reply(body) => MessageBodyResponse::Reply(body.into()),
            MessageBodyJson::RoomChange(body) => MessageBodyResponse::RoomChange(body.into()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextBodyResponse {
    pub text: String,
}

impl From<TextJson> for TextBodyResponse {
    fn from(stored: TextJson) -> Self {
        TextBodyResponse { text: stored.text }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MediaBodyResponse {
    pub media_url: String,
    pub media_type: String,
    pub mime_type: Option<String>,
    pub alt_text: Option<String>,
}

impl From<MediaJson> for MediaBodyResponse {
    fn from(stored: MediaJson) -> Self {
        MediaBodyResponse {
            media_url: stored.media_url,
            media_type: stored.media_type,
            mime_type: stored.mime_type,
            alt_text: stored.alt_text,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReplyBodyResponse {
    pub reply_msg_id: Uuid,
    pub reply_sender_id: Uuid,
    pub reply_msg_type: MsgType,
    pub reply_created_at: DateTime<Utc>,
    pub reply_msg_details: RepliedMessageResponse,
    pub reply_text: String,
}

impl From<ReplyJson> for ReplyBodyResponse {
    fn from(stored: ReplyJson) -> Self {
        ReplyBodyResponse {
            reply_msg_id: stored.reply_msg_id,
            reply_sender_id: stored.reply_sender_id,
            reply_msg_type: stored.reply_msg_type,
            reply_created_at: stored.reply_created_at,
            reply_msg_details: RepliedMessageResponse::from(stored.reply_msg_details),
            reply_text: stored.reply_text,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum RepliedMessageResponse {
    Text(TextBodyResponse),
    Media(MediaBodyResponse),
    Reply { reply_text: String },
}

impl From<RepliedMessageJson> for RepliedMessageResponse {
    fn from(stored: RepliedMessageJson) -> Self {
        match stored {
            RepliedMessageJson::Text(body) => RepliedMessageResponse::Text(body.into()),
            RepliedMessageJson::Media(body) => RepliedMessageResponse::Media(body.into()),
            RepliedMessageJson::Reply { reply_text } => RepliedMessageResponse::Reply { reply_text },
        }
    }
}

/// A membership change as rendered in the timeline.
///
/// `related_user` stays a [`RoomMemberSnapshotJson`] rather than becoming a `RoomMemberResponse`:
/// what the client shows here is who joined *at that moment*, which is exactly the frozen value the
/// row holds. Converting it to a live member type would imply a freshness the data does not have.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum RoomChangeResponse {
    UserJoined { related_user: RoomMemberSnapshotJson },
    UserLeft { related_user: RoomMemberSnapshotJson },
    UserInvited { related_user: RoomMemberSnapshotJson },
}

impl From<RoomChangeJson> for RoomChangeResponse {
    fn from(stored: RoomChangeJson) -> Self {
        match stored {
            RoomChangeJson::UserJoined { related_user } => RoomChangeResponse::UserJoined { related_user },
            RoomChangeJson::UserLeft { related_user } => RoomChangeResponse::UserLeft { related_user },
            RoomChangeJson::UserInvited { related_user } => RoomChangeResponse::UserInvited { related_user },
        }
    }
}

/// A page of the chat timeline: the messages plus the deduplicated profiles of every user that
/// authored a message in this page, or is the original author quoted by a reply.
///
/// Senders resolve even if they have since left the room, so the client can render every message
/// without a separate lookup. New live senders arrive embedded in the `ChatMessage` event.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePageResponse {
    pub messages: Vec<MessageResponse>,
    pub senders: Vec<RoomMemberResponse>,
}

impl ApiResponse for TimelinePageResponse {}

/// The caller's current position in their notification stream, for a client deciding whether it
/// needs to replay.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationCursorResponse {
    pub seq: u64,
}

impl ApiResponse for NotificationCursorResponse {}
