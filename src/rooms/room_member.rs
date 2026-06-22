use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A room participant. A row in `chat_room_participant` always means the user is
/// currently in the room — leaving deletes the row, so there is no membership state.
///
/// `joined_at` / `last_message_read_at` come from `chat_room_participant` and are
/// `None` for senders that are no longer members (e.g. historical message authors
/// surfaced in a timeline page after they left).
#[derive(Debug, Deserialize, Serialize, sqlx::FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RoomMember {
    pub id: Uuid,
    pub display_name: String,
    pub profile_picture: Option<String>,
    pub joined_at: Option<DateTime<Utc>>,
    pub last_message_read_at: Option<DateTime<Utc>>,
}

/// Cached per-room participant snapshot used for fast broadcast fan-out.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RoomContext {
    pub members: Vec<RoomMember>,
}

impl RoomContext {
    pub fn member_ids(&self) -> Vec<Uuid> {
        self.members.iter().map(|m| m.id).collect()
    }

    pub fn find_member(&self, user_id: &Uuid) -> Option<&RoomMember> {
        self.members.iter().find(|m| &m.id == user_id)
    }
}
