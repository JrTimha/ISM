//! The one error type the auth middleware produces, and how it reaches the client.
//!
//! Every variant is logged in full server-side and then sanitised by `classify` into the small
//! set of (status, `ErrorCode`, message) triples a caller is allowed to see — the reason a
//! token failed must not tell an attacker which check tripped.

use std::sync::Arc;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

use crate::auth::oidc_discovery;
use crate::core::errors::{ErrorCode, ErrorResponse};

/// Everything that can go wrong while authenticating a request.
#[derive(Debug, Clone, Error)]
pub enum AuthError {
    /// OIDC discovery never happened.
    #[error("Never discovered a OIDC configuration.")]
    NoOidcDiscovery,

    /// OIDC discovery failed.
    #[error("Could not discover OIDC configuration.")]
    OidcDiscovery {
        source: oidc_discovery::RequestError,
    },

    /// JWK set discovery never happened.
    #[error("Never discovered a JWK set.")]
    NoJwkSetDiscovery,

    /// JWK endpoint was not a valid URL.
    #[error("Could not parse the JWK endpoint.")]
    JwkEndpoint { source: url::ParseError },

    /// JWK set discovery failed.
    #[error("Could not discover the JWK set.")]
    JwkSetDiscovery {
        source: oidc_discovery::RequestError,
    },

    /// The 'Authorization' header was not present on a request.
    #[error("The 'Authorization' header was not present on a request.")]
    MissingAuthorizationHeader,

    /// The 'Authorization' header was present on a request but its value could not be parsed.
    /// This can occur if the header value did not solely contain visible ASCII characters.
    #[error(
        "The 'Authorization' header was present on a request but its value could not be parsed. Reason: {reason}"
    )]
    InvalidAuthorizationHeader { reason: String },

    /// The 'Authorization' header was present  and could be parsed, but it did not contain the expected "Bearer {token}" format.
    #[error("The 'Authorization' header did not contain the expected 'Bearer ...token' format.")]
    MissingBearerToken,

    /// No query parameters were found on the request.
    #[error("No query parameters were found on the request.")]
    MissingQueryParams,

    /// Query parameters were found on the request, but the expected token parameter wasn't.
    #[error("Query parameters were found on the request, but the expected token parameter wasn't.")]
    MissingTokenQueryParam,

    /// Query parameters were found on the request, and the expected token parameter was found, but it had no value assigned ("?token=").
    #[error(
        "Query parameters were found on the request, and the expected token parameter was found, but it had no value assigned (\"?token=\")."
    )]
    EmptyTokenQueryParam,

    /// No JWT could be extracted from the request.
    #[error("No JWT could be extracted from the request.")]
    MissingToken,

    /// The DecodingKey, required for decoding tokens, could not be created.
    #[error(
        "The DecodingKey, required for decoding tokens, could not be created. Source: {source}"
    )]
    CreateDecodingKey { source: jsonwebtoken::errors::Error },

    /// The JWT header could not be decoded.
    #[error("The JWT header could not be decoded. Source: {source}")]
    DecodeHeader { source: jsonwebtoken::errors::Error },

    /// No decoding keys were fetched jet.
    #[error("There were no decoding keys available.")]
    NoDecodingKeys,

    /// The JWT could not be decoded.
    #[error("The JWT could not be decoded. Source: {source}")]
    Decode { source: jsonwebtoken::errors::Error },

    /// Parts of the JWT could not be parsed.
    #[error("Parts of the JWT could not be parsed. Source: {source}")]
    JsonParse { source: Arc<serde_json::Error> },

    /// The tokens lifetime is expired.
    #[error("The tokens lifetime is expired.")]
    TokenExpired,

    /// For a not further known reason, the token was deemed invalid
    #[error("For a not further known reason, the token was deemed invalid: Reason: {reason}")]
    InvalidToken { reason: String },

    /// Note: The role is only ever logged server-side, never returned to the client.
    #[error("An expected role was missing: {role}")]
    MissingExpectedRole { role: String },

    /// An unexpected role was present.
    #[error("An unexpected role was present.")]
    UnexpectedRole,
}

/// Renders an error together with its full `source` chain as `outer: middle: inner`.
///
/// Several variants above deliberately keep their own `Display` free of the source (so the
/// sanitised client message and the log line can differ), which means a plain `{err}` would drop
/// the actual cause. Used for the discovery failures logged in `instance.rs`.
pub(crate) fn error_chain(err: &dyn std::error::Error) -> String {
    let mut chain = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}

impl AuthError {
    /// Stable, low-cardinality discriminant used as a structured log field.
    fn kind(&self) -> &'static str {
        match self {
            AuthError::NoOidcDiscovery => "no_oidc_discovery",
            AuthError::OidcDiscovery { .. } => "oidc_discovery",
            AuthError::NoJwkSetDiscovery => "no_jwk_set_discovery",
            AuthError::JwkEndpoint { .. } => "jwk_endpoint",
            AuthError::JwkSetDiscovery { .. } => "jwk_set_discovery",
            AuthError::MissingAuthorizationHeader => "missing_authorization_header",
            AuthError::InvalidAuthorizationHeader { .. } => "invalid_authorization_header",
            AuthError::MissingBearerToken => "missing_bearer_token",
            AuthError::MissingQueryParams => "missing_query_params",
            AuthError::MissingTokenQueryParam => "missing_token_query_param",
            AuthError::EmptyTokenQueryParam => "empty_token_query_param",
            AuthError::MissingToken => "missing_token",
            AuthError::CreateDecodingKey { .. } => "create_decoding_key",
            AuthError::DecodeHeader { .. } => "decode_header",
            AuthError::NoDecodingKeys => "no_decoding_keys",
            AuthError::Decode { .. } => "decode",
            AuthError::JsonParse { .. } => "json_parse",
            AuthError::TokenExpired => "token_expired",
            AuthError::InvalidToken { .. } => "invalid_token",
            AuthError::MissingExpectedRole { .. } => "missing_expected_role",
            AuthError::UnexpectedRole => "unexpected_role",
        }
    }

    /// Maps the error onto its client-visible representation.
    ///
    /// The message deliberately never depends on the underlying source. Reporting *why* a token
    /// was rejected — bad signature vs. wrong audience vs. expired — turns this endpoint into an
    /// oracle for crafting tokens, so all rejections of the same class are indistinguishable.
    /// The full detail is logged server-side instead.
    fn classify(&self) -> (StatusCode, ErrorCode, &'static str) {
        const UNAVAILABLE_MSG: &str = "Authentication service unavailable. Please try again later.";
        const UNAUTHORIZED_MSG: &str = "Authentication required.";

        match self {
            // The identity provider is unreachable or misconfigured — this is our fault, not the
            // caller's, and 503 matches how `AppError::Database`/`Cache` signal upstream trouble.
            AuthError::NoOidcDiscovery
            | AuthError::OidcDiscovery { .. }
            | AuthError::NoJwkSetDiscovery
            | AuthError::JwkEndpoint { .. }
            | AuthError::JwkSetDiscovery { .. }
            | AuthError::CreateDecodingKey { .. }
            | AuthError::NoDecodingKeys => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::AuthUnavailable,
                UNAVAILABLE_MSG,
            ),

            AuthError::TokenExpired => (
                StatusCode::UNAUTHORIZED,
                ErrorCode::TokenExpired,
                "Token expired.",
            ),

            // Everything from "no header" through "signature did not verify" collapses into one
            // indistinguishable 401. `DecodeHeader` used to be a 400, which leaked that the token
            // was malformed rather than merely invalid.
            AuthError::MissingAuthorizationHeader
            | AuthError::InvalidAuthorizationHeader { .. }
            | AuthError::MissingBearerToken
            | AuthError::MissingQueryParams
            | AuthError::MissingTokenQueryParam
            | AuthError::EmptyTokenQueryParam
            | AuthError::MissingToken
            | AuthError::DecodeHeader { .. }
            | AuthError::Decode { .. }
            | AuthError::InvalidToken { .. } => (
                StatusCode::UNAUTHORIZED,
                ErrorCode::Unauthorized,
                UNAUTHORIZED_MSG,
            ),

            AuthError::MissingExpectedRole { .. } | AuthError::UnexpectedRole => (
                StatusCode::FORBIDDEN,
                ErrorCode::InsufficientPermissions,
                "Insufficient permissions.",
            ),

            AuthError::JsonParse { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::UnexpectedError,
                "An unexpected error occurred.",
            ),
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = self.classify();

        // Full detail stays server-side; the client only ever sees the sanitised message above.
        if status.is_server_error() {
            tracing::error!(error.kind = self.kind(), error = %self, "Authentication failed");
        } else {
            tracing::debug!(error.kind = self.kind(), error = %self, "Rejected request");
        }

        let body = ErrorResponse::new(status, error_code, message);
        (status, Json(body)).into_response()
    }
}
