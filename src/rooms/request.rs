//! Client-supplied inputs for the rooms domain.

use crate::core::ApiRequest;
use crate::core::cursor::PageSize;
use crate::messaging::request::FirstMessageRequest;
use crate::rooms::model::RoomType;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;
use validator::{Validate, ValidationError};

/// Body of `POST /api/v1/rooms/create-room`.
///
/// Until this type existed only `first_message` was validated, and the handler had to remember to
/// do it by hand: `room_name` was unbounded and `invited_users` had no length cap, so a single
/// request could push an arbitrarily long name and tens of thousands of ids into the participant
/// insert. The bounds below run automatically now, via
/// [`ValidatedJson`](crate::core::ValidatedJson).
///
/// What is *not* checked here is anything needing a database read — whether the pair already has a
/// 1-1 room, and who has blocked the creator. Those stay in `RoomService`, where they hold for
/// every caller rather than only for this endpoint.
#[derive(Debug, Deserialize, Clone, Validate)]
#[serde(rename_all = "camelCase")]
#[validate(schema(function = "check_room_cardinality", skip_on_field_errors = true))]
pub struct NewRoomRequest {
    pub room_type: RoomType,
    #[validate(length(min = 1, max = 100, message = "must be between 1 and 100 characters long."))]
    pub room_name: Option<String>,
    /// Everyone the room starts with, including the creator.
    #[validate(length(min = 1, max = 50, message = "must contain between 1 and 50 users."))]
    pub invited_users: Vec<Uuid>,
    /// Optional first message sent together with the room. Only `Text` or `Media` (a link to a
    /// post) — never a `Reply`, since the room starts empty. Embedded into the `NewRoom` broadcast
    /// event so recipients render it without a lookup.
    #[serde(default)]
    #[validate(nested)]
    pub first_message: Option<FirstMessageRequest>,
}

impl ApiRequest for NewRoomRequest {}

/// A `Single` room is exactly two people: the creator and one other.
///
/// Cardinality is a property of what a room *is*, and it is decidable from the payload alone — no
/// query involved — so it is syntax and belongs here. `RoomService` still enforces the parts that
/// are not: that the creator is among the invitees (after blocked users are filtered out, which can
/// change the count) and that the pair has no existing 1-1 room.
///
/// A `Group` is deliberately **not** required to have a name. Unnamed groups exist and are handled
/// throughout — `ShareTargetResponse::name` is an `Option` precisely because of them, and the
/// joined-rooms query `COALESCE`s a null `room_name` against the other participant's display name.
/// Requiring one here would have rejected requests the server has always accepted.
fn check_room_cardinality(request: &NewRoomRequest) -> Result<(), ValidationError> {
    if request.room_type == RoomType::Single && request.invited_users.len() != 2 {
        return Err(ValidationError::new("single_room_needs_exactly_two_users"));
    }
    Ok(())
}

/// Query params for `GET /api/v1/rooms` and `GET /api/v1/rooms/share-targets`.
#[derive(Debug, Deserialize, Validate)]
pub struct RoomListQuery {
    /// Optional case-insensitive name filter (the other user for single rooms, the room name for
    /// groups). Bounded because it reaches an `ILIKE '%…%'` pattern.
    #[validate(length(min = 1, max = 100, message = "must be between 1 and 100 characters long."))]
    pub name: Option<String>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: PageSize,
}

impl ApiRequest for RoomListQuery {}

/// Query params for `GET /api/v1/rooms/search`.
#[derive(Debug, Deserialize, Validate)]
pub struct RoomSearchQuery {
    #[serde(rename = "withUser")]
    pub with_user: Uuid,
}

impl ApiRequest for RoomSearchQuery {}

/// Query params for `GET /api/v1/rooms/{room_id}/timeline`.
///
/// The timeline pages backwards from a timestamp rather than through an opaque cursor, because
/// `chat_message` is keyset-indexed on `(chat_room_id, created_at DESC)` and the client already
/// holds the timestamp of its oldest loaded message.
#[derive(Debug, Deserialize, Validate)]
pub struct TimelineQuery {
    pub timestamp: DateTime<Utc>,
}

impl ApiRequest for TimelineQuery {}
