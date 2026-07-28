//! Strategies for locating the raw JWT in an incoming request.
//!
//! The layer holds a non-empty list of these and uses the first one that yields a token. ISM
//! keeps the default (`Authorization: Bearer …`) only.

use std::{borrow::Cow, sync::Arc};

use axum::extract::Request;
use nonempty::NonEmpty;
use url::form_urlencoded;

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

        // First occurrence wins, matching how a duplicated parameter was resolved before.
        let (_, token) = form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key.as_ref() == self.key.as_str())
            .ok_or(AuthError::MissingTokenQueryParam)?;

        if token.is_empty() {
            return Err(AuthError::EmptyTokenQueryParam);
        }

        // `form_urlencoded` decodes lossily, so `?token=%FF` arrives as U+FFFD instead of failing.
        // A JWT is base64url segments joined by dots and therefore pure ASCII, so anything outside
        // that range is rejected here rather than handed to the decoder as a silently mangled
        // string. (The previous implementation returned raw bytes and had to guard UTF-8 itself —
        // before that it `expect`ed, which made a malformed query string a remote panic.)
        if !token.is_ascii() {
            return Err(AuthError::InvalidToken {
                reason: "token query parameter contained non-ASCII characters".to_owned(),
            });
        }

        // Borrowed when the value needed no unescaping, owned otherwise — see `ExtractedToken`.
        Ok(token)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use axum::body::Body;

    fn request(uri: &str) -> Request {
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("valid test request")
    }

    #[test]
    fn reads_the_configured_query_parameter() {
        let request = request("http://localhost/api/v1/wss?token=abc.def.ghi");
        let token = QueryParamTokenExtractor::default()
            .extract(&request)
            .expect("token to be extracted");
        assert_eq!(token, "abc.def.ghi");
    }

    #[test]
    fn percent_decodes_the_value() {
        let request = request("http://localhost/?token=a%2Eb");
        let token = QueryParamTokenExtractor::default()
            .extract(&request)
            .expect("token to be extracted");
        assert_eq!(token, "a.b");
    }

    #[test]
    fn ignores_other_parameters_and_takes_the_first_duplicate() {
        let request = request("http://localhost/?last_seq=7&token=first&token=second");
        let token = QueryParamTokenExtractor::default()
            .extract(&request)
            .expect("token to be extracted");
        assert_eq!(token, "first");
    }

    #[test]
    fn honours_a_custom_key() {
        let request = request("http://localhost/?token=wrong&jwt=right");
        let token = QueryParamTokenExtractor::extracting_key("jwt")
            .extract(&request)
            .expect("token to be extracted");
        assert_eq!(token, "right");
    }

    #[test]
    fn distinguishes_absent_missing_and_empty_parameters() {
        let no_query = request("http://localhost/api/v1/wss");
        assert!(matches!(
            QueryParamTokenExtractor::default().extract(&no_query),
            Err(AuthError::MissingQueryParams)
        ));

        let other_key = request("http://localhost/?last_seq=7");
        assert!(matches!(
            QueryParamTokenExtractor::default().extract(&other_key),
            Err(AuthError::MissingTokenQueryParam)
        ));

        let empty_value = request("http://localhost/?token=");
        assert!(matches!(
            QueryParamTokenExtractor::default().extract(&empty_value),
            Err(AuthError::EmptyTokenQueryParam)
        ));
    }

    #[test]
    fn rejects_a_non_ascii_token_instead_of_mangling_it() {
        // `%FF` is not valid UTF-8; percent-decoding it lossily yields U+FFFD, which must not be
        // passed on to the JWT decoder as if it were the token the caller sent.
        let request = request("http://localhost/?token=%FF");
        assert!(matches!(
            QueryParamTokenExtractor::default().extract(&request),
            Err(AuthError::InvalidToken { .. })
        ));
    }

    #[test]
    fn header_extractor_requires_the_bearer_prefix() {
        let bearer = Request::builder()
            .uri("http://localhost/")
            .header(http::header::AUTHORIZATION, "Bearer abc.def.ghi")
            .body(Body::empty())
            .expect("valid test request");
        assert_eq!(
            AuthHeaderTokenExtractor {}
                .extract(&bearer)
                .expect("token to be extracted"),
            "abc.def.ghi"
        );

        let basic = Request::builder()
            .uri("http://localhost/")
            .header(http::header::AUTHORIZATION, "Basic abc")
            .body(Body::empty())
            .expect("valid test request");
        assert!(matches!(
            AuthHeaderTokenExtractor {}.extract(&basic),
            Err(AuthError::MissingBearerToken)
        ));

        let none = request("http://localhost/");
        assert!(matches!(
            AuthHeaderTokenExtractor {}.extract(&none),
            Err(AuthError::MissingAuthorizationHeader)
        ));
    }

    #[test]
    fn extract_jwt_falls_through_to_the_next_extractor() {
        let extractors: NonEmpty<Arc<dyn TokenExtractor>> = NonEmpty {
            head: Arc::new(AuthHeaderTokenExtractor {}),
            tail: vec![Arc::new(QueryParamTokenExtractor::default())],
        };

        let with_query = request("http://localhost/api/v1/wss?token=from.query");
        assert_eq!(
            extract_jwt(&with_query, &extractors).as_deref(),
            Some("from.query")
        );

        let neither = request("http://localhost/api/v1/wss");
        assert_eq!(extract_jwt(&neither, &extractors), None);
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
