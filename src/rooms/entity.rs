//! Database rows and JSONB column payloads for the rooms domain.
//!
//! Two categories live here. `…Row` types are what a `SELECT` produces and carry no serde at all.
//! `…Json` types *are* serde — their `Serialize`/`Deserialize` impls are the storage format of a
//! `jsonb` column, which is why they are kept apart from the response types they resemble: a field
//! renamed on a response is an API change, the same rename here stops every existing row decoding.

use crate::core::{DbRow, JsonColumn};
use crate::rooms::model::{RoomChangeType, RoomType};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use uuid::Uuid;

/// A row of `chat_room`, plus two values computed per caller.
///
/// `room_name` / `room_image_url` are `COALESCE`d: for a `Single` room they are the *other*
/// participant's name and avatar, for a `Group` the room's own. `unread` is derived from the
/// caller's `last_message_read_at` and is `None` for queries made outside any caller's context
/// (`select_room`), which is why it is an `Option` rather than a `bool`.
#[derive(Debug, sqlx::FromRow)]
pub struct ChatRoomRow {
    pub id: Uuid,
    pub room_type: RoomType,
    pub room_name: Option<String>,
    pub room_image_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub latest_message: Option<DateTime<Utc>>,
    pub latest_message_preview_text: Option<Json<LastMessagePreviewJson>>,
    pub unread: Option<bool>,
}

impl DbRow for ChatRoomRow {}

/// A room participant.
///
/// A row in `chat_room_participant` always means the user is currently in the room — leaving
/// deletes the row, so there is no membership state. `joined_at` / `last_message_read_at` are
/// `None` for users who are no longer members but still appear as historical message authors in a
/// timeline page, which the `LEFT JOIN` in `select_message_senders` produces.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RoomMemberRow {
    pub id: Uuid,
    pub display_name: String,
    pub profile_picture: Option<String>,
    pub joined_at: Option<DateTime<Utc>>,
    pub last_message_read_at: Option<DateTime<Utc>>,
}

impl DbRow for RoomMemberRow {}

/// A room member as frozen into a stored `chat_message.msg_body` room-change record.
///
/// **Storage format**, and the reason it is not just [`RoomMemberRow`] or `RoomMemberResponse`: it
/// is a snapshot of who joined or left *at the time it happened*, written once and never updated.
/// A live member type is free to gain and lose fields; this one cannot, because every historical
/// `RoomChange` message in `chat_message` is already encoded against it.
///
/// The shape below is exactly what has been written until now, including `joined_at` and
/// `last_message_read_at` — values that are meaningless in a frozen snapshot but are present in
/// existing rows and on the wire. Trimming them needs a data migration, not a struct edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomMemberSnapshotJson {
    pub id: Uuid,
    pub display_name: String,
    pub profile_picture: Option<String>,
    pub joined_at: Option<DateTime<Utc>>,
    pub last_message_read_at: Option<DateTime<Utc>>,
}

impl JsonColumn for RoomMemberSnapshotJson {}

impl From<RoomMemberRow> for RoomMemberSnapshotJson {
    fn from(row: RoomMemberRow) -> Self {
        RoomMemberSnapshotJson {
            id: row.id,
            display_name: row.display_name,
            profile_picture: row.profile_picture,
            joined_at: row.joined_at,
            last_message_read_at: row.last_message_read_at,
        }
    }
}

/// The stored value of `chat_room.latest_message_preview_text`.
///
/// **Storage format.** The `type` tag and the snake_case field names below are what is written in
/// every existing row; changing either makes those rows undecodable. The client-facing counterpart
/// is [`LastMessagePreviewResponse`](crate::rooms::response::LastMessagePreviewResponse), which is
/// free to differ.
///
/// Note what is *absent*: the `#[serde(serialize_with = "truncate_and_serialize")]` that used to sit
/// on `text` and `reply_text`. Because this type was also the response type, that display rule ran
/// on the `INSERT` path too and the database has been storing pre-truncated previews. Truncation
/// now happens in the conversion to the response, so what is stored is the full text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LastMessagePreviewJson {
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
    /// A room that has no messages yet.
    New,
}

impl JsonColumn for LastMessagePreviewJson {}

/// Row of the *active* share-target section: a room the client can already send to — either a group
/// room or a friend's existing 1-1 room. Ordered by `active_at DESC`. `user_id` is the friend
/// behind a 1-1 room (`None` for groups); the share target is always the existing `room_id`.
/// Populated by [`RoomRepository::active_share_targets`](crate::rooms::RoomRepository::active_share_targets).
#[derive(Debug, sqlx::FromRow)]
pub struct ActiveShareRow {
    /// Display name of the other user (1-1) or the room name (group); groups may be unnamed.
    pub name: Option<String>,
    pub room_id: Uuid,
    pub image_url: Option<String>,
    pub active_at: DateTime<Utc>,
    pub is_group: bool,
    pub user_id: Option<Uuid>,
}

impl DbRow for ActiveShareRow {}

/// Row of the *inactive* share-target section: a friend the client has no 1-1 room with yet.
/// Ordered by `display_name ASC`. Sharing requires creating the room first. Populated by
/// [`RoomRepository::inactive_share_targets`](crate::rooms::RoomRepository::inactive_share_targets).
#[derive(Debug, sqlx::FromRow)]
pub struct InactiveShareRow {
    pub name: String,
    pub user_id: Uuid,
    pub image_url: Option<String>,
}

impl DbRow for InactiveShareRow {}

#[cfg(test)]
mod convention_guards {
    //! See `core::model` for why this is written as a compile-time `impls!` assertion rather than a
    //! trait bound. Note that `LastMessagePreviewJson` is deliberately *not* listed: it is a
    //! [`JsonColumn`], and serde is its whole purpose.

    use super::*;
    use impls::impls;
    use serde::Serialize;

    const _: () = assert!(!impls!(ChatRoomRow: Serialize));
    const _: () = assert!(!impls!(RoomMemberRow: Serialize));
    const _: () = assert!(!impls!(ActiveShareRow: Serialize));
    const _: () = assert!(!impls!(InactiveShareRow: Serialize));

    // The storage type must keep both halves of its serde contract, or existing rows stop decoding.
    const _: () = assert!(impls!(LastMessagePreviewJson: Serialize));
    const _: () = assert!(impls!(LastMessagePreviewJson: serde::de::DeserializeOwned));
}
