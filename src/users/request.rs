//! Client-supplied inputs for the users domain.
//!
//! Both types are extracted with [`ValidatedQuery`](crate::core::ValidatedQuery), so the bounds
//! below run before a handler body starts. `limit` needs no bound of its own: [`PageSize`] clamps
//! during deserialization, so an out-of-range value is capped at `MAX_PAGE_SIZE` rather than
//! rejected — asking for more than the server serves is not a malformed request.

use crate::core::ApiRequest;
use crate::core::cursor::PageSize;
use serde::Deserialize;
use validator::Validate;

/// Query params for `GET /api/v1/users/search`.
#[derive(Debug, Deserialize, Validate)]
pub struct UserSearchQuery {
    /// Case-insensitive substring matched against the indexed `raw_name` column.
    ///
    /// Bounded because the value reaches a `LIKE '%…%'` pattern: an empty needle matches the whole
    /// table and an unbounded one lets a client push arbitrary bytes into the query planner.
    #[validate(length(min = 1, max = 100, message = "must be between 1 and 100 characters long."))]
    pub username: String,
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: PageSize,
}

impl ApiRequest for UserSearchQuery {}

/// Query params for `GET /api/v1/users/friends` and `GET /api/v1/users/friends/requests`.
///
/// Same shape as [`UserSearchQuery`] except that the name filter is optional — omitting it lists
/// everything rather than searching.
#[derive(Debug, Deserialize, Validate)]
pub struct FriendListQuery {
    #[validate(length(min = 1, max = 100, message = "must be between 1 and 100 characters long."))]
    pub username: Option<String>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: PageSize,
}

impl ApiRequest for FriendListQuery {}
