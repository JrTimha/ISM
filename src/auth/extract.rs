//! Strategies for locating the raw JWT in an incoming request.
//!
//! The layer holds a non-empty list of these and uses the first one that yields a token. ISM
//! keeps the default (`Authorization: Bearer …`) only.

use std::{borrow::Cow, sync::Arc};

use axum::extract::Request;
use nonempty::NonEmpty;

use crate::auth::error::AuthError;

/// A raw (unprocessed) token (string) taken from a request.
/// This being `Cow` allows the `TokenExtractor` implementations to borrow from the request if possible.
pub type ExtractedToken<'a> = Cow<'a, str>;

/// Allows for customized strategies on how to retrieve the auth token from an axum request.
/// This crate implements two default strategies:
///   - `AuthHeaderTokenExtractor`: Extracts the token from the `http::header::AUTHORIZATION` header.
///   - `QueryParamTokenExtractor`: Extracts the token from a query parameter (for example named "token").
///
/// Note: The current return type and caller impl does not allow to return multiple tokens from a request.
/// We may implement this feature in the future. This could allow the QueryParamTokenExtractor to extract all tokens found.
pub trait TokenExtractor: Send + Sync + std::fmt::Debug {
    fn extract<'a>(&self, request: &'a Request) -> Result<ExtractedToken<'a>, AuthError>;
}

/// Searches the auth token in the authorization header. (Authorization: `Bearer <token>`)
#[derive(Debug, Clone, Default)]
pub struct AuthHeaderTokenExtractor {}

impl TokenExtractor for AuthHeaderTokenExtractor {
    fn extract<'a>(&self, request: &'a Request) -> Result<ExtractedToken<'a>, AuthError> {
        request
            .headers()
            .get(http::header::AUTHORIZATION)
            .ok_or(AuthError::MissingAuthorizationHeader)?
            .to_str()
            .map_err(|err| AuthError::InvalidAuthorizationHeader {
                reason: err.to_string(),
            })?
            .strip_prefix("Bearer ")
            .ok_or(AuthError::MissingBearerToken)
            .map(Cow::Borrowed)
    }
}

/// Searches the auth token in the query parameters, eg. returns `<token>` when looking at a request with URL `https://<url>/<path>?token=<token>`.
/// The key to be searched for is configurable. Default is: "token".
///
/// SECURITY: This extractor should be used with caution!
/// Only use it if you are informed about the security implication of providing tokens through query parameters.
///
/// Not wired up in ISM — the layer uses the header extractor only. Kept because a browser
/// `WebSocket` cannot set request headers, so authenticating `/api/wss` will need this or an
/// equivalent. See `docs/auth.md`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct QueryParamTokenExtractor {
    /// Name of the query parameter carrying the token.
    pub key: String,
}

impl QueryParamTokenExtractor {
    /// Builds an extractor reading the token from the query parameter named `key`.
    pub fn extracting_key(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }
}

impl Default for QueryParamTokenExtractor {
    fn default() -> Self {
        Self::extracting_key("token")
    }
}

impl TokenExtractor for QueryParamTokenExtractor {
    fn extract<'a>(&self, request: &'a Request) -> Result<ExtractedToken<'a>, AuthError> {
        let query = request.uri().query().ok_or(AuthError::MissingQueryParams)?;

        let mut tokens = serde_querystring::DuplicateQS::parse(query.as_bytes())
            .values(self.key.as_bytes())
            .unwrap_or_default()
            .into_iter();

        let first_token = tokens
            .next()
            .ok_or(AuthError::MissingTokenQueryParam)?
            .ok_or(AuthError::EmptyTokenQueryParam)?;

        // Percent-decoding yields raw bytes, so `?token=%FF` is not necessarily valid UTF-8.
        // This used to `expect`, which made a malformed query string a remote panic.
        let first_token =
            std::str::from_utf8(first_token.as_ref()).map_err(|_| AuthError::InvalidToken {
                reason: "token query parameter was not valid UTF-8".to_owned(),
            })?;

        Ok(ExtractedToken::Owned(first_token.to_owned()))
    }
}

/// Returns the token from the first extractor that succeeds, or `None` if all of them fail.
pub fn extract_jwt<'a>(
    request: &'a Request<axum::body::Body>,
    extractors: &NonEmpty<Arc<dyn TokenExtractor>>,
) -> Option<ExtractedToken<'a>> {
    for extractor in extractors {
        match extractor.extract(request) {
            Ok(jwt) => return Some(jwt),
            Err(err) => {
                tracing::debug!(?extractor, ?err, "Extractor failed");
            }
        }
    }
    None
}
