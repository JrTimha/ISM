use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UserDTO {
    pub id: Uuid,
    pub display_name: String,
    pub street_credits: u32,
    pub profile_picture: Option<String>,
    pub friends_count: u32,
    pub description: Option<String>
}