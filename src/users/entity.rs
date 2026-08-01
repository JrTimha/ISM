//! Database rows for `app_user` and `user_relationship`.
//!
//! Nothing here derives `Serialize`, and the `convention_guards` module at the bottom proves it.
//! That is what lets [`UserRow`] carry `email`, `deleted_at` and the audit timestamps: the row is
//! the shape of the table, not the shape of an endpoint, so reading a column costs nothing in
//! exposure. What a client sees is [`UserProfileResponse`](crate::users::response::UserProfileResponse),
//! built from a row by an explicit `From`.

use crate::core::DbRow;
use crate::users::model::RelationshipState;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, Row};
use uuid::Uuid;

/// A row of `app_user`.
///
/// `app_user` is shared with the wider Meventure platform, so it carries columns ISM never reads
/// (`home_location`, `finished_intro`, …). The fields below are the ones ISM needs; the last five
/// are backend-only and must not appear in any response.
#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub display_name: String,
    pub street_credits: i64,
    pub profile_picture: Option<String>,
    pub description: Option<String>,
    pub friends_count: i64,
    pub posts_count: i64,
    pub role: String,

    // ── Backend-only ────────────────────────────────────────────────────────
    /// Personal data. Present so ISM can address a user (push, moderation) without a second
    /// query; never rendered to another user.
    pub email: String,
    pub created_at: DateTime<Utc>,
    /// Soft-delete marker, set by the platform rather than by ISM.
    ///
    /// Every query that *offers* a user — search, friends, friend requests, share targets — filters
    /// on `deleted_at IS NULL`. Room membership and message authorship deliberately do not: see
    /// [`RoomRepository::select_all_room_member`](crate::rooms::RoomRepository::select_all_room_member).
    ///
    /// Relationships pointing at a deleted user are **not** cleaned up here; another service
    /// dissolves them. That is exactly why the filter has to live on the read path — the rows
    /// outlive the account.
    pub deleted_at: Option<DateTime<Utc>>,
    pub last_modified_at: Option<DateTime<Utc>>,
    /// Lower-cased `display_name`, maintained by the platform and backed by the `user_rawname`
    /// index. It exists to make `LIKE` searches indexable and is meaningless to a client.
    pub raw_name: Option<String>,
}

impl DbRow for UserRow {}

impl UserRow {
    /// Whether this user has been soft-deleted.
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// A row of `user_relationship`.
///
/// The table is symmetric: one row per pair, stored with `user_a_id < user_b_id`, and the
/// direction lives in [`RelationshipState`]. Which side the *caller* is on is therefore not a
/// property of the row — see
/// [`Relationship::for_viewer`](crate::users::response::Relationship::for_viewer).
#[derive(Debug, Clone)]
pub struct UserRelationshipRow {
    pub user_a_id: Uuid,
    pub user_b_id: Uuid,
    pub state: RelationshipState,
    pub relationship_change_timestamp: DateTime<Utc>,
}

impl DbRow for UserRelationshipRow {}

/// A user joined with the relationship they have to the caller, if any.
///
/// The relationship columns arrive from a `LEFT JOIN`, so they are all-or-nothing: either the
/// four are present or there is no relationship. [`Self::relationship`] is what re-imposes that
/// invariant on the four independent `Option`s.
#[derive(Debug)]
pub struct UserWithRelationshipRow {
    pub user: UserRow,
    user_a_id: Option<Uuid>,
    user_b_id: Option<Uuid>,
    relationship_state: Option<RelationshipState>,
    relationship_change_timestamp: Option<DateTime<Utc>>,
}

impl DbRow for UserWithRelationshipRow {}

impl UserWithRelationshipRow {
    /// Reassembles the joined columns into a relationship row, or `None` when the `LEFT JOIN`
    /// found nothing.
    ///
    /// Matching all four `Option`s at once is what makes this total: the previous version tested
    /// `is_some()` four times and then called `unwrap()` four times, which is the same thing said
    /// twice and only correct as long as the two lists stay in step.
    pub fn relationship(&self) -> Option<UserRelationshipRow> {
        match (self.user_a_id, self.user_b_id, self.relationship_state, self.relationship_change_timestamp) {
            (Some(user_a_id), Some(user_b_id), Some(state), Some(relationship_change_timestamp)) => Some(UserRelationshipRow {
                user_a_id,
                user_b_id,
                state,
                relationship_change_timestamp,
            }),
            _ => None,
        }
    }
}

/// Hand-written because the row is a join of two tables: the `app_user` half is delegated to
/// `UserRow`'s derived impl, and `state` arrives as `text` that has to go through
/// [`RelationshipState::try_from`] rather than a `sqlx::Type` decode.
impl<'r, R: Row> FromRow<'r, R> for UserWithRelationshipRow
where
    &'r str: sqlx::ColumnIndex<R>,
    Uuid: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    String: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    i64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    DateTime<Utc>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    Option<Uuid>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    Option<String>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    Option<DateTime<Utc>>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    fn from_row(row: &'r R) -> Result<Self, sqlx::Error> {
        let user = UserRow::from_row(row)?;
        let state_str: Option<String> = row.try_get("state")?;

        let relationship_state: Option<RelationshipState> = state_str
            .map(RelationshipState::try_from)
            .transpose()
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;

        Ok(UserWithRelationshipRow {
            user,
            user_a_id: row.try_get("user_a_id")?,
            user_b_id: row.try_get("user_b_id")?,
            relationship_state,
            relationship_change_timestamp: row.try_get("relationship_change_timestamp")?,
        })
    }
}

#[cfg(test)]
mod convention_guards {
    //! Rust has no negative trait bounds, so "this type must not implement `Serialize`" cannot be
    //! a where-clause. `impls!` answers it at compile time instead; the mechanism itself is proven
    //! in `core::model`. A `#[derive(Serialize)]` added to any row below fails the build here.

    use super::*;
    use impls::impls;
    use serde::Serialize;

    const _: () = assert!(!impls!(UserRow: Serialize));
    const _: () = assert!(!impls!(UserRelationshipRow: Serialize));
    const _: () = assert!(!impls!(UserWithRelationshipRow: Serialize));
}
