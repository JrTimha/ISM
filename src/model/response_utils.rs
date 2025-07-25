use serde::{Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResponse {
    pub image_url: String,
    pub image_name: String
}