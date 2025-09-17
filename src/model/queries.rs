use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize, Debug)]
pub struct SingleRoomSearchUserParams {
    #[serde(rename = "withUser")]
    pub with_user: Uuid
}