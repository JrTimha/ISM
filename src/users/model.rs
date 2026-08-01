//! Types shared by more than one boundary in the users domain.
//!
//! [`RelationshipState`] is the stored `user_relationship.state` value; the pagination cursor is
//! an opaque client token. Neither is a row, a request or a response, so neither belongs in
//! `entity.rs`, `request.rs` or `response.rs`.

use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::error::Error;
use std::fmt;
use std::fmt::{Display, Formatter};
use uuid::Uuid;

/// The stored state of a relationship, from the table's own point of view.
///
/// `A` and `B` refer to the row's `user_a_id` / `user_b_id`, which are ordered by id and have
/// nothing to do with who is asking. Turning this into something a client can read requires the
/// viewer's id — see [`Relationship::for_viewer`](crate::users::response::Relationship::for_viewer).
///
/// The column is `varchar` with a `CHECK` constraint rather than a Postgres enum, which is why
/// values are bound through [`Display`] and read back through [`TryFrom<String>`].
#[allow(non_camel_case_types)]
#[derive(Debug, Deserialize, Serialize, Clone, Type, PartialEq, Copy)]
#[sqlx(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationshipState {
    A_BLOCKED,
    B_BLOCKED,
    ALL_BLOCKED,
    FRIEND,
    A_INVITED,
    B_INVITED,
}

/// A `user_relationship.state` value the `CHECK` constraint should have made impossible.
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
        let value = match self {
            RelationshipState::FRIEND => "FRIEND",
            RelationshipState::B_BLOCKED => "B_BLOCKED",
            RelationshipState::A_BLOCKED => "A_BLOCKED",
            RelationshipState::ALL_BLOCKED => "ALL_BLOCKED",
            RelationshipState::A_INVITED => "A_INVITED",
            RelationshipState::B_INVITED => "B_INVITED",
        };
        write!(f, "{value}")
    }
}

/// Keyset cursor for every user list: search, friends and friend requests.
///
/// Ordered by `(display_name, id)` ascending, with `id` as the deterministic tie-breaker for
/// duplicate display names.
#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserPaginationCursor {
    pub last_seen_name: Option<String>,
    pub last_seen_id: Option<Uuid>,
}
