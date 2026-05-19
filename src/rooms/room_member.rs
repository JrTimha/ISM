use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RoomMemberContext {
    pub user_id: Uuid,
    pub display_name: String,
    pub allow_read_receipts: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RoomContext {
    pub members: Vec<RoomMemberContext>,
}

impl RoomContext {
    pub fn member_ids(&self) -> Vec<Uuid> {
        self.members.iter().map(|m| m.user_id).collect()
    }

    pub fn find_member(&self, user_id: &Uuid) -> Option<&RoomMemberContext> {
        self.members.iter().find(|m| &m.user_id == user_id)
    }
}

#[derive(Debug, Deserialize, Serialize, sqlx::FromRow, sqlx::Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RoomMember {
    pub id: Uuid,
    pub display_name: String,
    pub profile_picture: Option<String>,
    pub joined_at: DateTime<Utc>,
    pub last_message_read_at: Option<DateTime<Utc>>,
    pub membership_status: MembershipStatus
}

#[derive(Debug, Deserialize, Serialize, Clone, Type, PartialEq)]
#[sqlx(type_name = "membership_status")]
pub enum MembershipStatus {
    Joined,
    Left,
    Invited
}

impl MembershipStatus {

    pub fn to_str(&self) -> &str {
        match self {
            MembershipStatus::Joined => "Joined",
            MembershipStatus::Left => "Left",
            MembershipStatus::Invited => "Invited"

        }
    }

    pub fn to_string(&self) -> String {
        match self {
            MembershipStatus::Joined => String::from("Joined"),
            MembershipStatus::Left => String::from("Left"),
            MembershipStatus::Invited => String::from("Invited")
        }
    }
}