//! Types the rooms domain shares across boundaries.
//!
//! Enums that are simultaneously a database value and a wire value ([`RoomType`],
//! [`RoomChangeType`]), the opaque pagination cursors, and the Redis cache shape. None of them is a
//! row, a request or a response, so none belongs in `entity.rs`, `request.rs` or `response.rs`.

use crate::rooms::entity::RoomMemberRow;
use crate::rooms::response::RoomMemberResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::fmt::{self, Display, Formatter};
use uuid::Uuid;

/// Whether a room is a 1-1 conversation or a named group.
///
/// Stored in `chat_room.room_type` as `varchar` with a `CHECK` constraint (not a Postgres enum,
/// despite the `type_name` below), which is why writes bind it through [`Display`].
#[derive(Debug, Deserialize, Serialize, Clone, Copy, Type, PartialEq)]
#[sqlx(type_name = "room_type")]
pub enum RoomType {
    Single,
    Group,
}

impl Display for RoomType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let value = match self {
            RoomType::Single => "Single",
            RoomType::Group => "Group",
        };
        write!(f, "{value}")
    }
}

/// What happened to a room's membership, as recorded in a preview text.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
pub enum RoomChangeType {
    LEAVE,
    JOIN,
    INVITE,
}

/// Keyset cursor for the joined-rooms list. Rooms are ordered by recent activity
/// (`latest_message DESC`) with `id` as a deterministic tie-breaker.
#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RoomPaginationCursor {
    pub last_seen_latest_message: Option<DateTime<Utc>>,
    pub last_seen_room_id: Option<Uuid>,
}

/// Which section of the merged share list the next page resumes in. The list is two-phase: active
/// rooms first, then inactive friends.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SharePhase {
    /// Rooms with activity (groups + friends with an existing 1-1 room), `active_at DESC`.
    #[default]
    Active,
    /// Friends without a 1-1 room, `displayName ASC`.
    Inactive,
}

/// Keyset cursor for the two-phase share-target list. The active section paginates over
/// `(active_at, room_id) DESC`; once it is exhausted the inactive section paginates over
/// `(name, user_id) ASC`. `phase` records which section the next page resumes in; the default
/// (`Active`, no bounds) starts at the top of the list.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareTargetCursor {
    pub phase: SharePhase,
    pub last_active_at: Option<DateTime<Utc>>,
    pub last_name: Option<String>,
    pub last_id: Option<Uuid>,
}

/// Cached per-room participant snapshot used for fast broadcast fan-out.
///
/// Holds [`RoomMemberResponse`] rather than a dedicated cache struct, which is a deliberate
/// exception to the rule that separates storage shapes from wire shapes. A cache is a *disposable*
/// projection: if the shape ever needs to change, the cost is a miss and a rebuild from the
/// database — bump the `room_context:` prefix in `cache::util` and the old entries are simply
/// ignored. A `jsonb` column has no such escape, which is why [`LastMessagePreviewJson`] does get
/// its own type. Reuse where a mistake is recoverable; split where it is not.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RoomContext {
    pub members: Vec<RoomMemberResponse>,
}

impl RoomContext {
    /// Builds the snapshot from freshly read rows.
    pub fn from_rows(rows: Vec<RoomMemberRow>) -> Self {
        RoomContext {
            members: rows.into_iter().map(RoomMemberResponse::from).collect(),
        }
    }

    pub fn member_ids(&self) -> Vec<Uuid> {
        self.members.iter().map(|m| m.id).collect()
    }

    pub fn find_member(&self, user_id: &Uuid) -> Option<&RoomMemberResponse> {
        self.members.iter().find(|m| &m.id == user_id)
    }
}
