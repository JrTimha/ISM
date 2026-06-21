use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::messaging::model::MessageDto;
use crate::rooms::room::{ChatRoomDto, LastMessagePreviewText};
use crate::rooms::room_member::RoomMember;
use crate::users::model::User;

/// Current wire-format version of the streaming envelope. Bump on breaking changes.
pub const NOTIFICATION_VERSION: u8 = 1;

fn default_version() -> u8 {
    NOTIFICATION_VERSION
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// Envelope version, allows the client to evolve its parser safely.
    #[serde(default = "default_version")]
    pub v: u8,
    /// Monotonic per-user sequence number. `None` for ephemeral events (e.g. typing)
    /// and when sequencing is unavailable (no Redis). Used by clients to detect gaps
    /// and resume after a reconnect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(flatten)]
    pub body: NotificationEvent,
    pub created_at: DateTime<Utc>
}

impl Notification {
    /// Build a fresh notification with the current envelope version, no sequence
    /// number (assigned later per-user in the broadcast layer), and the current time.
    pub fn new(body: NotificationEvent) -> Self {
        Notification {
            v: NOTIFICATION_VERSION,
            seq: None,
            body,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum NotificationEvent {

    #[serde(rename_all = "camelCase")]
    FriendRequestReceived {from_user: User},

    #[serde(rename_all = "camelCase")]
    FriendRequestAccepted {from_user: User},

    /**
    * Different chat messages, sent to all active users in a room. `sender` carries the
    * message author's profile so clients can render a first-time sender without a
    * separate lookup (the timeline page bundles historical senders the same way).
    */
    #[serde(rename_all = "camelCase")]
    ChatMessage {message: MessageDto, room_preview_text: LastMessagePreviewText, sender: RoomMember },

    /**
    * A system message is a message not sent by a user, but by the system, whatever you want
    */
    SystemMessage {message: serde_json::Value},

    /**
    * Sending this event to a newly invited user
    */
    #[serde(rename_all = "camelCase")]
    NewRoom {room: ChatRoomDto, created_by: User },

    /**
    * Sending this event to a user who has left a room
    */
    #[serde(rename_all = "camelCase")]
    LeaveRoom {room_id: Uuid},

    /**
    * Sending this event to all users in a room where a member has left
    */
    #[serde(rename_all = "camelCase")]
    RoomChangeEvent {message: MessageDto, room_preview_text: LastMessagePreviewText},

    /**
    * Sending this event to all users in a room when a user has read the latest message
    */
    #[serde(rename_all = "camelCase")]
    UserReadChat {user_id: Uuid, room_id: Uuid},

    /**
    * Control event: the client's last known sequence is too old to be replayed from the
    * cache (gap larger than the retention window, or events lost while lagging). The client
    * must re-fetch the authoritative state via REST (timeline / friends / rooms) and then
    * continue consuming live events. Always ephemeral: never sequenced, never cached.
    */
    #[serde(rename_all = "camelCase")]
    Resync {reason: String}
}

impl NotificationEvent {
    /// Ephemeral events are delivered live-only: they never receive a sequence number and
    /// are never cached for replay. A typing indicator from 30 minutes ago is irrelevant,
    /// so re-delivering it after a reconnect would be wrong. Durable events (the default)
    /// are sequenced and cached so a reconnecting client can catch up without loss.
    pub fn is_ephemeral(&self) -> bool {
        match self {
            NotificationEvent::Resync { .. } => true,
            NotificationEvent::FriendRequestReceived { .. }
            | NotificationEvent::FriendRequestAccepted { .. }
            | NotificationEvent::ChatMessage { .. }
            | NotificationEvent::SystemMessage { .. }
            | NotificationEvent::NewRoom { .. }
            | NotificationEvent::LeaveRoom { .. }
            | NotificationEvent::RoomChangeEvent { .. }
            | NotificationEvent::UserReadChat { .. } => false,
        }
    }
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SendNotification {
    #[serde(flatten)]
    pub body: Notification,
    pub to_user: Uuid
}
