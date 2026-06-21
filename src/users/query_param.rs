use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct UserSearchParams {
    pub username: String,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// Query params for the paginated friends / friend-requests lists.
/// `username` is an optional case-insensitive name filter.
#[derive(Deserialize, Debug)]
pub struct RelationshipQueryParams {
    pub username: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}