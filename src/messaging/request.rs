//! Client-supplied inputs for the messaging domain.
//!
//! Kept apart from the storage types even though several bodies look identical to them today: a
//! request is what a client is *allowed to say*, a `…Json` is what the server chose to persist.
//! `ReplyBodyRequest` is the clearest case — a client sends the id it is replying to and its own
//! text, while the stored `ReplyJson` additionally carries a frozen copy of the quoted message that
//! the server resolved. One type could not honestly do both jobs.

use crate::core::ApiRequest;
use crate::messaging::entity::{MediaJson, MessageBodyJson, TextJson};
use crate::messaging::model::MsgType;
use serde::Deserialize;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

/// Body of `POST /api/v1/send-msg`.
#[derive(Debug, Deserialize, Clone, Validate)]
#[serde(rename_all = "camelCase")]
#[validate(schema(function = "check_msg_type_matches_body", skip_on_field_errors = true))]
pub struct SendMessageRequest {
    pub chat_room_id: Uuid,
    #[validate(nested)]
    pub msg_body: SendMessageBodyRequest,
    /// Redundant with `msg_body`'s shape, but part of the accepted request format. It is checked
    /// against the body rather than trusted — see [`check_msg_type_matches_body`] — and then
    /// discarded: `MessageRow::new` derives the stored `msg_type` from the body itself.
    pub msg_type: MsgType,
}

impl ApiRequest for SendMessageRequest {}

/// Rejects a payload whose declared `msgType` contradicts the body it carries.
///
/// The two were never compared before, so a client could label a media body as `Text`. Nothing
/// downstream trusted the field, but it was echoed back in the response and cached in the
/// notification stream, which meant one client's mislabelling became every client's problem.
fn check_msg_type_matches_body(request: &SendMessageRequest) -> Result<(), ValidationError> {
    let implied = match request.msg_body {
        SendMessageBodyRequest::Text(_) => MsgType::Text,
        SendMessageBodyRequest::Media(_) => MsgType::Media,
        SendMessageBodyRequest::Reply(_) => MsgType::Reply,
    };
    if implied == request.msg_type {
        Ok(())
    } else {
        Err(ValidationError::new("msg_type_does_not_match_msg_body"))
    }
}

/// The body a client may send. `untagged`, so the variant is recovered from the field names.
///
/// No `RoomChange` variant: membership messages are written by the server, never posted.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum SendMessageBodyRequest {
    Text(TextBodyRequest),
    Media(MediaBodyRequest),
    Reply(ReplyBodyRequest),
}

/// Hand-written because `#[derive(Validate)]` does not cover enums; it forwards to whichever
/// variant was deserialized so `#[validate(nested)]` on the parent still reaches the bounds.
impl Validate for SendMessageBodyRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            SendMessageBodyRequest::Text(body) => body.validate(),
            SendMessageBodyRequest::Media(body) => body.validate(),
            SendMessageBodyRequest::Reply(body) => body.validate(),
        }
    }
}

impl From<SendMessageBodyRequest> for MessageBodyJson {
    /// Only total for `Text` and `Media`. A `Reply` needs the quoted message resolved from the
    /// database first, so `MessageService::create_reply_message` builds that variant instead — the
    /// reason this returns `MessageBodyJson` for two of three cases and the service handles the
    /// third.
    fn from(request: SendMessageBodyRequest) -> Self {
        match request {
            SendMessageBodyRequest::Text(body) => MessageBodyJson::Text(TextJson { text: body.text }),
            SendMessageBodyRequest::Media(body) => MessageBodyJson::Media(MediaJson {
                media_url: body.media_url,
                media_type: body.media_type,
                mime_type: body.mime_type,
                alt_text: body.alt_text,
            }),
            // Unreachable in practice: the service intercepts `Reply` before this conversion. The
            // fallback stores the text alone rather than panicking on a path that cannot be hit.
            SendMessageBodyRequest::Reply(body) => MessageBodyJson::Text(TextJson { text: body.reply_text }),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Validate)]
#[serde(rename_all = "camelCase")]
pub struct TextBodyRequest {
    #[validate(length(min = 1, max = 4000, message = "must be between 1 and 4000 characters long."))]
    pub text: String,
}

#[derive(Debug, Deserialize, Clone, Validate)]
#[serde(rename_all = "camelCase")]
pub struct MediaBodyRequest {
    #[validate(length(min = 1, max = 250, message = "must be between 1 and 250 characters long."))]
    pub media_url: String,
    #[validate(length(min = 1, max = 80, message = "must be between 1 and 80 characters long."))]
    pub media_type: String,
    /// Bounded like the rest: both this and `alt_text` were previously unconstrained, so a client
    /// could store an arbitrarily large string inside every message's `jsonb` body.
    #[validate(length(max = 255, message = "must be at most 255 characters long."))]
    pub mime_type: Option<String>,
    #[validate(length(max = 1000, message = "must be at most 1000 characters long."))]
    pub alt_text: Option<String>,
}

/// What a client sends to reply: the message being replied to, and the reply text. Everything else
/// on the stored [`ReplyJson`](crate::messaging::entity::ReplyJson) is resolved server-side.
#[derive(Debug, Deserialize, Clone, Validate)]
#[serde(rename_all = "camelCase")]
pub struct ReplyBodyRequest {
    pub reply_msg_id: Uuid,
    #[validate(length(min = 1, max = 4000, message = "must be between 1 and 4000 characters long."))]
    pub reply_text: String,
}

/// Body of the optional first message that can be sent together with a new room.
///
/// A brand-new room has no prior messages, so a `Reply` is impossible here — only `Text` and
/// `Media` are valid. `chat_room_id` is intentionally absent: the room does not exist yet.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum FirstMessageRequest {
    Text(TextBodyRequest),
    Media(MediaBodyRequest),
}

impl Validate for FirstMessageRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            FirstMessageRequest::Text(body) => body.validate(),
            FirstMessageRequest::Media(body) => body.validate(),
        }
    }
}

impl From<FirstMessageRequest> for MessageBodyJson {
    fn from(request: FirstMessageRequest) -> Self {
        match request {
            FirstMessageRequest::Text(body) => MessageBodyJson::Text(TextJson { text: body.text }),
            FirstMessageRequest::Media(body) => MessageBodyJson::Media(MediaJson {
                media_url: body.media_url,
                media_type: body.media_type,
                mime_type: body.mime_type,
                alt_text: body.alt_text,
            }),
        }
    }
}

/// Query params for the SSE and WebSocket handshakes.
///
/// `last_seq` is the highest sequence number the client already has; the server replays what came
/// after it, or emits a `Resync` when the gap has been trimmed out of the retained window.
#[derive(Debug, Deserialize, Validate)]
pub struct StreamHandshakeQuery {
    #[serde(default)]
    pub last_seq: Option<u64>,
}

impl ApiRequest for StreamHandshakeQuery {}

/// Query params for `GET /api/v1/notifications`.
#[derive(Debug, Deserialize, Validate)]
pub struct NotificationBacklogQuery {
    pub last_seq: u64,
}

impl ApiRequest for NotificationBacklogQuery {}
