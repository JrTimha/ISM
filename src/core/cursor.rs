use std::fmt;
use base64::Engine;
use base64::engine::general_purpose;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub trait Cursor: Serialize + DeserializeOwned + Default {}
impl<T> Cursor for T where T: Serialize + DeserializeOwned + Default {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorResults<T> {
    pub next_cursor: Option<String>,
    pub content: Vec<T>,
}

pub fn decode_cursor<T: Cursor>(base64_cursor: Option<String>) -> Result<T, CursorError> {
    match base64_cursor {
        Some(encoded_cursor) => {
            let decoded_bytes = general_purpose::URL_SAFE_NO_PAD.decode(encoded_cursor.as_bytes())?;
            let cursor: T = serde_json::from_slice(&decoded_bytes)?;
            Ok(cursor)
        },
        None => {
            Ok(T::default())
        }
    }
}

pub fn encode_cursor<T: Cursor>(cursor: &T) -> Result<String, CursorError> {
    let json_bytes = serde_json::to_vec(cursor)?;
    let encoded_cursor = general_purpose::URL_SAFE_NO_PAD.encode(&json_bytes);
    Ok(encoded_cursor)
}

#[derive(Debug)]
pub enum CursorError {
    Base64Decode(base64::DecodeError),
    Json(serde_json::Error),
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CursorError::Base64Decode(_) => write!(f, "Ungültiger Base64-Cursor"),
            CursorError::Json(_) => write!(f, "Cursor-Daten konnten nicht als JSON verarbeitet werden"),
        }
    }
}

impl std::error::Error for CursorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CursorError::Base64Decode(e) => Some(e),
            CursorError::Json(e) => Some(e),
        }
    }
}

impl From<base64::DecodeError> for CursorError {
    fn from(err: base64::DecodeError) -> Self {
        CursorError::Base64Decode(err)
    }
}

impl From<serde_json::Error> for CursorError {
    fn from(err: serde_json::Error) -> Self {
        CursorError::Json(err)
    }
}