use base64::Engine;
use base64::engine::general_purpose;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt;

pub trait Cursor: Serialize + DeserializeOwned + Default {}
impl<T> Cursor for T where T: Serialize + DeserializeOwned + Default {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorResults<T> {
    pub cursor: Option<String>,
    pub content: Vec<T>,
}

pub fn decode_cursor<T: Cursor>(base64_cursor: Option<String>) -> Result<T, CursorError> {
    match base64_cursor {
        Some(encoded_cursor) => {
            let decoded_bytes =
                general_purpose::URL_SAFE_NO_PAD.decode(encoded_cursor.as_bytes())?;
            let cursor: T = serde_json::from_slice(&decoded_bytes)?;
            Ok(cursor)
        }
        None => Ok(T::default()),
    }
}

pub fn encode_cursor<T: Cursor>(cursor: &T) -> Result<String, CursorError> {
    let json_bytes = serde_json::to_vec(cursor)?;
    let encoded_cursor = general_purpose::URL_SAFE_NO_PAD.encode(&json_bytes);
    Ok(encoded_cursor)
}

/// Default number of items returned per page when the client omits `limit`.
pub const DEFAULT_PAGE_SIZE: usize = 20;
/// Upper bound for a client-supplied `limit` — prevents unbounded page sizes.
pub const MAX_PAGE_SIZE: usize = 50;

/// Clamps a client-supplied page size into `[1, MAX_PAGE_SIZE]`, defaulting to
/// `DEFAULT_PAGE_SIZE` when the value is missing or zero.
pub fn clamp_page_size(requested: Option<u32>) -> usize {
    match requested {
        Some(n) if n >= 1 => (n as usize).min(MAX_PAGE_SIZE),
        _ => DEFAULT_PAGE_SIZE,
    }
}

/// Finalizes a keyset page. Callers fetch `page_size + 1` rows; this truncates the
/// slice back to `page_size` and, if there were more rows, encodes the continuation
/// cursor derived from the last item of the returned page.
pub fn next_cursor<T, C, F>(
    items: &mut Vec<T>,
    page_size: usize,
    cursor_from: F,
) -> Result<Option<String>, CursorError>
where
    C: Cursor,
    F: FnOnce(&T) -> C,
{
    if items.len() > page_size {
        items.truncate(page_size);
        match items.last() {
            Some(last) => Ok(Some(encode_cursor(&cursor_from(last))?)),
            None => Ok(None),
        }
    } else {
        Ok(None)
    }
}

#[derive(Debug)]
pub enum CursorError {
    Base64Decode(base64::DecodeError),
    Json(serde_json::Error),
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CursorError::Base64Decode(_) => write!(f, "Invalid base64 cursor"),
            CursorError::Json(_) => write!(f, "Failed to deserialize cursor data as JSON"),
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
