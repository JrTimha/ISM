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
pub const MAX_PAGE_SIZE: usize = 30;

/// Clamps a client-supplied page size into `[1, MAX_PAGE_SIZE]`, defaulting to
/// `DEFAULT_PAGE_SIZE` when the value is missing or zero.
fn clamp_page_size(requested: Option<u32>) -> usize {
    match requested {
        Some(n) if n >= 1 => (n as usize).min(MAX_PAGE_SIZE),
        _ => DEFAULT_PAGE_SIZE,
    }
}

/// A page size that is already clamped.
///
/// The clamping happens during **deserialization**, so a request type carrying this field cannot
/// reach a handler holding an out-of-range value, and a handler cannot forget to call
/// [`clamp_page_size`]. That call used to sit in five handlers; a sixth list endpoint that omitted
/// it would have passed a client-supplied `limit` straight into `LIMIT $n`.
///
/// Out-of-range is deliberately **not** a `400`. A client asking for 1000 items is asking for more
/// than the server serves, not making a malformed request — the documented contract in
/// `.claude/rules/pagination.md` is that the server clamps, and rejecting instead would break
/// clients that pass a large number to mean "as many as I can get". The bound is the server's
/// policy, not the client's error.
///
/// Use it as `#[serde(default)] pub limit: PageSize` and read it with [`PageSize::get`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSize(usize);

impl PageSize {
    /// The clamped value, always within `[1, MAX_PAGE_SIZE]`.
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for PageSize {
    /// Used when the client omits `limit` entirely, via `#[serde(default)]`.
    fn default() -> Self {
        PageSize(DEFAULT_PAGE_SIZE)
    }
}

impl<'de> serde::Deserialize<'de> for PageSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Through `u32` rather than `usize`: it rejects a negative `limit` with a deserialization
        // error on every target, instead of letting it depend on the platform's pointer width.
        let requested = u32::deserialize(deserializer)?;
        Ok(PageSize(clamp_page_size(Some(requested))))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Mirrors how a list endpoint declares its query parameters.
    #[derive(Debug, Deserialize)]
    struct Params {
        #[serde(default)]
        limit: PageSize,
    }

    fn parse(query: &str) -> Result<Params, serde_urlencoded::de::Error> {
        serde_urlencoded::from_str(query)
    }

    #[test]
    fn omitted_limit_falls_back_to_the_default() {
        assert_eq!(parse("").unwrap().limit.get(), DEFAULT_PAGE_SIZE);
        assert_eq!(parse("cursor=abc").unwrap().limit.get(), DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn a_limit_inside_the_range_is_kept() {
        // Bounds are derived from the constant, never written as literals: `MAX_PAGE_SIZE` is a
        // tuning knob, and a hardcoded `50` here would keep passing after it moved while quietly
        // testing clamping under a name that claims the opposite.
        assert_eq!(parse("limit=1").unwrap().limit.get(), 1);
        assert_eq!(parse(&format!("limit={MAX_PAGE_SIZE}")).unwrap().limit.get(), MAX_PAGE_SIZE);

        let mid = MAX_PAGE_SIZE / 2;
        assert_eq!(parse(&format!("limit={mid}")).unwrap().limit.get(), mid);
    }

    #[test]
    fn an_oversized_limit_is_clamped_rather_than_rejected() {
        // The point of the newtype: this is a served request, not a client error.
        for over in [MAX_PAGE_SIZE + 1, MAX_PAGE_SIZE * 100, u32::MAX as usize] {
            assert_eq!(
                parse(&format!("limit={over}")).unwrap().limit.get(),
                MAX_PAGE_SIZE,
                "limit={over} was not clamped"
            );
        }
    }

    // Both page-size constants are compile-time values, so this is a compile-time assertion rather
    // than a `#[test]`: a default above the cap would hand out more than the ceiling allows on every
    // request that omits `limit` — the one path where nothing clamps — and that should fail the
    // build, not wait for someone to run the suite.
    const _: () = assert!(DEFAULT_PAGE_SIZE >= 1);
    const _: () = assert!(
        DEFAULT_PAGE_SIZE <= MAX_PAGE_SIZE,
        "DEFAULT_PAGE_SIZE exceeds MAX_PAGE_SIZE: an omitted `limit` would return more than the cap"
    );

    #[test]
    fn zero_falls_back_to_the_default_instead_of_returning_nothing() {
        assert_eq!(parse("limit=0").unwrap().limit.get(), DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn a_negative_or_non_numeric_limit_is_a_deserialization_error() {
        // These are malformed, unlike an oversized value, so they surface as a 400 from the
        // extractor rather than being silently coerced.
        assert!(parse("limit=-1").is_err());
        assert!(parse("limit=abc").is_err());
        assert!(parse("limit=").is_err());
    }

    #[test]
    fn a_clamped_page_size_is_never_zero() {
        // `next_cursor` and every repository fetch `page_size + 1`; a zero would make the
        // look-ahead row the entire page and paginate forever.
        for query in ["", "limit=0", "limit=1", "limit=99999"] {
            assert!(parse(query).unwrap().limit.get() >= 1, "{query} produced an empty page size");
        }
    }
}
