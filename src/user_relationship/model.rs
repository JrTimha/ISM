use std::error::Error;
use std::fmt;
use std::fmt::{Display, Formatter};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, Type};
use uuid::Uuid;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FriendRequestResult {
    pub id: Uuid,
    pub from_user: User,
}

#[derive(Debug, Clone)]
pub struct UserRelationship {
    pub user_a_id: Uuid,
    pub user_b_id: Uuid,
    pub state: RelationshipState,
    pub relationship_change_timestamp: DateTime<Utc>
}

#[derive(Debug)]
pub struct UserWithRelationship {
    pub r_user: User,
    user_a_id: Option<Uuid>,
    user_b_id: Option<Uuid>,
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

impl<'r, R: Row> FromRow<'r, R> for UserWithRelationship
where
    &'r str: sqlx::ColumnIndex<R>,
    Uuid: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    String: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    i64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    DateTime<Utc>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{

    fn from_row(row: &'r R) -> Result<Self, sqlx::Error> {

        let r_user = User::from_row(row)?;
        let state_str: Option<String> = row.try_get("state")?;

        let relationship_state: Option<RelationshipState> = state_str
            .map(RelationshipState::try_from)
            .transpose()
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;

        let user_a_id = row.try_get("user_a_id")?;
        let user_b_id = row.try_get("user_b_id")?;
        let relationship_change_timestamp = row.try_get("relationship_change_timestamp")?;

        Ok(UserWithRelationship {
            r_user,
            user_a_id,
            user_b_id,
            relationship_state,
            relationship_change_timestamp,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserWithRelationshipDto {
    pub user: User,
    pub relationship_type: Option<Relationship>,
}



#[allow(non_camel_case_types)]
#[derive(Debug, Deserialize, Serialize, Clone, Type, PartialEq)]
#[sqlx(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationshipState {
    A_BLOCKED,
    B_BLOCKED,
    ALL_BLOCKED,
    FRIEND,
    A_INVITED,
    B_INVITED
}

#[derive(Debug)]
pub struct InvalidState(String);

impl fmt::Display for InvalidState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unknown RelationshipState-Value: '{}'", self.0)
    }
}
impl Error for InvalidState {}

impl TryFrom<String> for RelationshipState {

    type Error = InvalidState;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "A_BLOCKED" => Ok(Self::A_BLOCKED),
            "B_BLOCKED" => Ok(Self::B_BLOCKED),
            "ALL_BLOCKED" => Ok(Self::ALL_BLOCKED),
            "FRIEND" => Ok(Self::FRIEND),
            "A_INVITED" => Ok(Self::A_INVITED),
            "B_INVITED" => Ok(Self::B_INVITED),
            _ => Err(InvalidState(value)),
        }
    }
}

impl Display for RelationshipState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RelationshipState::FRIEND => write!(f, "FRIEND"),
            RelationshipState::B_BLOCKED => write!(f, "B_BLOCKED"),
            RelationshipState::A_BLOCKED => write!(f, "A_BLOCKED"),
            RelationshipState::ALL_BLOCKED => write!(f, "ALL_BLOCKED"),
            RelationshipState::A_INVITED => write!(f, "A_INVITED"),
            RelationshipState::B_INVITED => write!(f, "B_INVITED"),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
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

#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserPaginationCursor {
    pub last_seen_name: Option<String>,
    pub last_seen_id: Option<Uuid>,
}