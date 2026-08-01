//! Database rows and JSONB column payloads for the messaging domain.
//!
//! `chat_message.msg_body` is a `jsonb` column, so the `…Json` types below are a **storage
//! format**: their serde impls decode rows written months ago. The client-facing shapes in
//! `response.rs` mirror them field for field today and are free to stop doing so tomorrow; that
//! freedom is the entire reason the two families exist separately.
//!
//! The `#[serde(untagged)]` on [`MessageBodyJson`] is load-bearing and must stay: the discriminator
//! is the `msg_type` *column*, not anything inside the JSON, so the variant is recovered by shape
//! alone.

use crate::core::{DbRow, JsonColumn};
use crate::messaging::model::MsgType;
use crate::rooms::entity::RoomMemberSnapshotJson;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A row of `chat_message`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MessageRow {
    pub chat_room_id: Uuid,
    pub message_id: Uuid,
    pub sender_id: Uuid,
    pub msg_body: sqlx::types::Json<MessageBodyJson>,
    pub msg_type: MsgType,
    pub created_at: DateTime<Utc>,
}

impl DbRow for MessageRow {}

impl MessageRow {
    /// Builds a new message, deriving `msg_type` from the body it was given.
    ///
    /// The column and the body cannot disagree because nothing else sets `msg_type` — notably not
    /// the client, whose own `msgType` field is checked against its body during request validation
    /// and then discarded.
    pub fn new(room_id: Uuid, sender_id: Uuid, msg_body: MessageBodyJson) -> MessageRow {
        let msg_type = match &msg_body {
            MessageBodyJson::Text(_) => MsgType::Text,
            MessageBodyJson::Media(_) => MsgType::Media,
            MessageBodyJson::Reply(_) => MsgType::Reply,
            MessageBodyJson::RoomChange(_) => MsgType::RoomChange,
        };
        MessageRow {
            chat_room_id: room_id,
            message_id: Uuid::new_v4(),
            sender_id,
            msg_body: sqlx::types::Json(msg_body),
            msg_type,
            created_at: Utc::now(),
        }
    }
}

/// The stored value of `chat_message.msg_body`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageBodyJson {
    Text(TextJson),
    Media(MediaJson),
    Reply(ReplyJson),
    RoomChange(RoomChangeJson),
}

impl JsonColumn for MessageBodyJson {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextJson {
    pub text: String,
}

impl JsonColumn for TextJson {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJson {
    pub media_url: String,
    pub media_type: String,
    pub mime_type: Option<String>,
    pub alt_text: Option<String>,
}

impl JsonColumn for MediaJson {}

/// A reply, with a frozen copy of what it replied to.
///
/// The quoted fields are a snapshot on purpose: editing or deleting the original must not silently
/// rewrite every reply that quotes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyJson {
    pub reply_msg_id: Uuid,
    pub reply_sender_id: Uuid,
    pub reply_msg_type: MsgType,
    pub reply_created_at: DateTime<Utc>,
    pub reply_msg_details: RepliedMessageJson,
    pub reply_text: String,
}

impl JsonColumn for ReplyJson {}

/// The quoted content inside a [`ReplyJson`]. A reply to a reply keeps only the text, so the chain
/// does not nest without bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RepliedMessageJson {
    Text(TextJson),
    Media(MediaJson),
    Reply { reply_text: String },
}

impl JsonColumn for RepliedMessageJson {}

/// A membership change recorded in the timeline.
///
/// `related_user` is a [`RoomMemberSnapshotJson`] rather than a live member type — see that type
/// for why the shape is frozen.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RoomChangeJson {
    UserJoined { related_user: RoomMemberSnapshotJson },
    UserLeft { related_user: RoomMemberSnapshotJson },
    UserInvited { related_user: RoomMemberSnapshotJson },
}

impl JsonColumn for RoomChangeJson {}

#[cfg(test)]
mod convention_guards {
    //! See `core::model`. The `…Json` types are deliberately absent: serde is their purpose.

    use super::*;
    use impls::impls;
    use serde::Serialize;

    const _: () = assert!(!impls!(MessageRow: Serialize));

    // The storage types must keep both halves of their serde contract, or existing `msg_body`
    // values stop decoding.
    const _: () = assert!(impls!(MessageBodyJson: Serialize));
    const _: () = assert!(impls!(MessageBodyJson: serde::de::DeserializeOwned));
}
