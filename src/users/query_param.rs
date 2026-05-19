use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct UserSearchParams {
    pub username: String,
    pub cursor: Option<String>,
}