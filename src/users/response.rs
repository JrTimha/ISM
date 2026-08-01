//! Client-facing shapes for the users domain.
//!
//! Every type here is built from a row in `entity.rs` by an explicit conversion, which is the
//! point at which backend-only columns are dropped and a symmetric relationship row is resolved
//! into something written from the caller's point of view.

use crate::core::ApiResponse;
use crate::users::entity::{UserRelationshipRow, UserRow, UserWithRelationshipRow};
use crate::users::model::RelationshipState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A user profile as any other user sees it.
///
/// `street_credits`, `posts_count` and `role` are product surface, not internal bookkeeping: the
/// first two are the platform's gamification counters and `role` drives client-side affordances.
/// What is *not* here is everything on [`UserRow`] below `role` — email, audit timestamps, the
/// soft-delete marker and the search key.
///
/// `Deserialize` is present under the one exception in [`ApiResponse`]: this type is embedded in
/// `FriendRequestReceived`, `FriendRequestAccepted` and `NewRoom`, and those envelopes are read
/// back out of the Redis replay stream on reconnect.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UserProfileResponse {
    pub id: Uuid,
    pub display_name: String,
    pub street_credits: i64,
    pub profile_picture: Option<String>,
    pub description: Option<String>,
    pub friends_count: i64,
    pub posts_count: i64,
    pub role: String,
}

impl ApiResponse for UserProfileResponse {}

impl From<UserRow> for UserProfileResponse {
    fn from(row: UserRow) -> Self {
        UserProfileResponse {
            id: row.id,
            display_name: row.display_name,
            street_credits: row.street_credits,
            profile_picture: row.profile_picture,
            description: row.description,
            friends_count: row.friends_count,
            posts_count: row.posts_count,
            role: row.role,
        }
    }
}

impl From<&UserRow> for UserProfileResponse {
    fn from(row: &UserRow) -> Self {
        UserProfileResponse::from(row.clone())
    }
}

/// A relationship as the *caller* experiences it.
///
/// [`RelationshipState`] is stored from the row's perspective (`A_INVITED` says the row's
/// `user_a_id` sent the invite, which is meaningless without knowing who is asking). This enum is
/// the same fact rewritten for one viewer: the same row yields `InviteSent` to one participant and
/// `InviteReceived` to the other.
#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Relationship {
    InviteReceived,
    InviteSent,
    ClientBlocked,
    ClientGotBlocked,
    Friend,
}

impl ApiResponse for Relationship {}

impl Relationship {
    /// Resolves a stored relationship into the viewer's perspective.
    ///
    /// A named constructor rather than `From`, because the conversion needs a second input and
    /// `From::from` has nowhere to put it. See `.claude/rules/model.md`.
    pub fn for_viewer(relationship: &UserRelationshipRow, viewer_id: &Uuid) -> Relationship {
        let viewer_is_a = relationship.user_a_id == *viewer_id;
        let viewer_is_b = relationship.user_b_id == *viewer_id;

        match relationship.state {
            RelationshipState::FRIEND => Relationship::Friend,

            RelationshipState::A_BLOCKED => {
                if viewer_is_a {
                    Relationship::ClientBlocked
                } else {
                    Relationship::ClientGotBlocked
                }
            }

            RelationshipState::B_BLOCKED => {
                if viewer_is_b {
                    Relationship::ClientBlocked
                } else {
                    Relationship::ClientGotBlocked
                }
            }

            // Both sides blocked: whoever is asking has blocked the other.
            RelationshipState::ALL_BLOCKED => {
                if viewer_is_a || viewer_is_b {
                    Relationship::ClientBlocked
                } else {
                    Relationship::ClientGotBlocked
                }
            }

            RelationshipState::A_INVITED => {
                if viewer_is_a {
                    Relationship::InviteSent
                } else {
                    Relationship::InviteReceived
                }
            }

            RelationshipState::B_INVITED => {
                if viewer_is_b {
                    Relationship::InviteSent
                } else {
                    Relationship::InviteReceived
                }
            }
        }
    }
}

/// A user profile plus how the caller relates to them. `relationshipType` is `null` when there is
/// no relationship at all.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserWithRelationshipResponse {
    pub user: UserProfileResponse,
    pub relationship_type: Option<Relationship>,
}

impl ApiResponse for UserWithRelationshipResponse {}

impl UserWithRelationshipResponse {
    /// Builds the response from the joined row, resolving the relationship for `viewer_id`.
    pub fn for_viewer(row: &UserWithRelationshipRow, viewer_id: &Uuid) -> Self {
        UserWithRelationshipResponse {
            user: UserProfileResponse::from(&row.user),
            relationship_type: row.relationship().map(|rel| Relationship::for_viewer(&rel, viewer_id)),
        }
    }
}

/// The relationship state after a block / unblock. `state` is `null` when the call removed the
/// relationship entirely.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipStateResponse {
    pub state: Option<Relationship>,
}

impl ApiResponse for RelationshipStateResponse {}
