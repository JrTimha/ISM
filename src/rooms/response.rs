//! Client-facing shapes for the rooms domain.

use crate::core::ApiResponse;
use crate::rooms::entity::{ActiveShareRow, ChatRoomRow, InactiveShareRow, LastMessagePreviewJson, RoomMemberRow};
use crate::rooms::model::{RoomChangeType, RoomType};
use crate::utils::truncate_preview;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A room as it appears in a list or on its own.
///
/// `Deserialize` under the notification exception in [`ApiResponse`]: this type is embedded in
/// `NewRoom`, which round-trips through the Redis replay stream.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RoomResponse {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_image_url: Option<String>,
    pub room_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub latest_message: Option<DateTime<Utc>>,
    pub unread: Option<bool>,
    pub latest_message_preview_text: LastMessagePreviewResponse,
}

impl ApiResponse for RoomResponse {}

impl From<ChatRoomRow> for RoomResponse {
    fn from(row: ChatRoomRow) -> Self {
        RoomResponse {
            id: row.id,
            room_type: row.room_type,
            room_image_url: row.room_image_url,
            room_name: row.room_name,
            created_at: row.created_at,
            latest_message: row.latest_message,
            unread: row.unread,
            // A room with no messages has a NULL preview column; `New` is how that reads to a client.
            latest_message_preview_text: row
                .latest_message_preview_text
                .map(|json| LastMessagePreviewResponse::from(json.0))
                .unwrap_or(LastMessagePreviewResponse::New),
        }
    }
}

impl From<&ChatRoomRow> for RoomResponse {
    fn from(row: &ChatRoomRow) -> Self {
        RoomResponse {
            id: row.id,
            room_type: row.room_type,
            room_image_url: row.room_image_url.clone(),
            room_name: row.room_name.clone(),
            created_at: row.created_at,
            latest_message: row.latest_message,
            unread: row.unread,
            latest_message_preview_text: row
                .latest_message_preview_text
                .as_ref()
                .map(|json| LastMessagePreviewResponse::from(json.0.clone()))
                .unwrap_or(LastMessagePreviewResponse::New),
        }
    }
}

/// A room plus its current participants. The room is flattened, so the payload is a
/// [`RoomResponse`] with one extra `users` key.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomDetailResponse {
    #[serde(flatten)]
    pub room: RoomResponse,
    pub users: Vec<RoomMemberResponse>,
}

impl ApiResponse for RoomDetailResponse {}

/// A room participant as a client sees them.
///
/// `Deserialize` under the notification exception: embedded in `ChatMessage.sender` and cached in
/// [`RoomContext`](crate::rooms::model::RoomContext).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RoomMemberResponse {
    pub id: Uuid,
    pub display_name: String,
    pub profile_picture: Option<String>,
    pub joined_at: Option<DateTime<Utc>>,
    pub last_message_read_at: Option<DateTime<Utc>>,
}

impl ApiResponse for RoomMemberResponse {}

impl From<RoomMemberRow> for RoomMemberResponse {
    fn from(row: RoomMemberRow) -> Self {
        RoomMemberResponse {
            id: row.id,
            display_name: row.display_name,
            profile_picture: row.profile_picture,
            joined_at: row.joined_at,
            last_message_read_at: row.last_message_read_at,
        }
    }
}

/// The one-line summary shown under a room in the room list.
///
/// The counterpart of [`LastMessagePreviewJson`], and the reason the two are separate types: the
/// long-text shortening applied here used to be a `serialize_with` on the shared type, so it also
/// ran when the value was written to `chat_room.latest_message_preview_text`. Display rules belong
/// on this side of the conversion.
///
/// Field names stay snake_case (`sender_username`, `media_type`, …) because the enum carries no
/// `rename_all` — matching what clients already parse.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum LastMessagePreviewResponse {
    Text {
        sender_username: String,
        text: String,
    },
    Media {
        sender_username: String,
        media_type: String,
    },
    Reply {
        sender_username: String,
        reply_text: String,
    },
    RoomChange {
        sender_username: String,
        room_change_type: RoomChangeType,
    },
    New,
}

impl ApiResponse for LastMessagePreviewResponse {}

impl From<LastMessagePreviewJson> for LastMessagePreviewResponse {
    fn from(stored: LastMessagePreviewJson) -> Self {
        match stored {
            LastMessagePreviewJson::Text { sender_username, text } => LastMessagePreviewResponse::Text {
                sender_username,
                text: truncate_preview(&text),
            },
            LastMessagePreviewJson::Media { sender_username, media_type } => LastMessagePreviewResponse::Media { sender_username, media_type },
            LastMessagePreviewJson::Reply { sender_username, reply_text } => LastMessagePreviewResponse::Reply {
                sender_username,
                reply_text: truncate_preview(&reply_text),
            },
            LastMessagePreviewJson::RoomChange {
                sender_username,
                room_change_type,
            } => LastMessagePreviewResponse::RoomChange {
                sender_username,
                room_change_type,
            },
            LastMessagePreviewJson::New => LastMessagePreviewResponse::New,
        }
    }
}

/// Result of a room image upload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomImageUploadResponse {
    pub image_url: String,
    pub image_name: String,
}

impl ApiResponse for RoomImageUploadResponse {}

/// A single suggestion of where the client can send shared content (like an Instagram "share to
/// chat" sheet). Merges friends and group rooms into one list; `target` tells the client whether to
/// post into an existing room or to create one first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareTargetResponse {
    /// Other user's display name (1-1) or the room name (group); a group may be unnamed.
    pub name: Option<String>,
    pub image_url: Option<String>,
    pub target: ShareTargetRef,
}

impl ApiResponse for ShareTargetResponse {}

/// What the client must do to deliver content to a [`ShareTargetResponse`].
///
/// The `rename_all` applies to the *variant* names only — serde does not propagate it into
/// struct-variant fields — so the payload is `{"kind":"room","room_id":…,"room_type":…}` with
/// snake_case keys inside an otherwise camelCase response. Inconsistent, but it is what clients
/// parse today; `tests/wire_contract.rs` pins it so it cannot be "tidied" by accident.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ShareTargetRef {
    /// An existing room — share via `POST /api/v1/send-msg` with this `room_id`.
    Room { room_id: Uuid, room_type: RoomType },
    /// A friend without a 1-1 room yet — create it via `POST /api/v1/rooms/create-room` for this
    /// `user_id`, then send into the returned room.
    User { user_id: Uuid },
}

impl From<ActiveShareRow> for ShareTargetResponse {
    fn from(row: ActiveShareRow) -> Self {
        let room_type = if row.is_group { RoomType::Group } else { RoomType::Single };
        ShareTargetResponse {
            name: row.name,
            image_url: row.image_url,
            target: ShareTargetRef::Room {
                room_id: row.room_id,
                room_type,
            },
        }
    }
}

impl From<InactiveShareRow> for ShareTargetResponse {
    fn from(row: InactiveShareRow) -> Self {
        ShareTargetResponse {
            name: Some(row.name),
            image_url: row.image_url,
            target: ShareTargetRef::User { user_id: row.user_id },
        }
    }
}
