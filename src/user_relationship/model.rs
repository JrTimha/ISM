use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Type};
use uuid::Uuid;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FriendRequestResult {
    pub id: Uuid,
    pub from_user: User,
}

#[derive(Debug, Deserialize, Serialize, Clone, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UserRelationship {
    pub user_a_id: Uuid,
    pub user_b_id: Uuid,
    pub state: RelationshipState,
    pub relationship_change_timestamp: DateTime<Utc>
}

#[derive(Debug, FromRow)]
pub struct UserWithRelationship {
    #[sqlx(flatten)]
    pub r_user: User,

    user_a_id: Option<Uuid>,
    user_b_id: Option<Uuid>,
    #[sqlx(rename = "state")]
    relationship_state: Option<RelationshipState>,
    relationship_change_timestamp: Option<DateTime<Utc>>,
}

impl UserWithRelationship {
    pub fn get_relationship(&self) -> Option<UserRelationship> {
        if self.user_a_id.is_some() && self.user_b_id.is_some() && self.relationship_state.is_some() && self.relationship_change_timestamp.is_some() {
            Some(UserRelationship {
                user_a_id: self.user_a_id.unwrap(),
                user_b_id: self.user_b_id.unwrap(),
                state: self.relationship_state.clone().unwrap(),
                relationship_change_timestamp: self.relationship_change_timestamp.unwrap(),
            })
        } else {
            None
        }
    }
}

#[derive(Serialize)]
pub struct UserWithRelationshipDto {
    pub user: User,
    pub relationship_type: Option<Relationship>,
}

#[allow(non_camel_case_types)]
#[derive(Debug, Deserialize, Serialize, Clone, Type, PartialEq)]
#[sqlx(type_name = "state")]
pub enum RelationshipState {
    A_BLOCKED,
    B_BLOCKED,
    ALL_BLOCKED,
    FRIEND,
    A_INVITED,
    B_INVITED
}

#[derive(Serialize)]
pub enum Relationship {
    InviteReceived,
    InviteSent,
    ClientBlocked,
    ClientGotBlocked,
    Friend
}

#[derive(Debug, Serialize, Deserialize, Clone, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
    pub street_credits: i64,
    pub profile_picture: Option<String>,
    pub description: Option<String>,
    pub friends_count: i64
}