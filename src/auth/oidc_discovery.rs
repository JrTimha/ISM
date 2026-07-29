//! The HTTP calls that fetch Keycloak's discovery document and JWK set.
//!
//! The documents themselves are modelled in `oidc.rs`; this module only does the fetching.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::auth::oidc::OidcConfig;
use reqwest::{IntoUrl, Response, StatusCode};
use serde::Deserialize;
use thiserror::Error;
use tracing::warn;

/// A discovery request that never produced a usable document.
#[derive(Debug, Clone, Error)]
pub enum RequestError {
    #[error("RequestError: Could not send request")]
    Send { source: Arc<reqwest::Error> },

    /// The server answered, but not with a document. Separate from `Decode` because the two mean
    /// entirely different things to whoever reads the log: this one says the request never reached
    /// a working endpoint — wrong realm, Keycloak not up yet, a proxy answering in its place.
    #[error("RequestError: Server answered with HTTP {status}: {body}")]
    Status { status: StatusCode, body: String },

    #[error("RequestError: Could not decode payload")]
    Decode { source: Arc<reqwest::Error> },
}

impl RequestError {
    fn send(source: reqwest::Error) -> Self {
        Self::Send {
            source: Arc::new(source),
        }
    }

    fn decode(source: reqwest::Error) -> Self {
        Self::Decode {
            source: Arc::new(source),
        }
    }
}

/// Hard deadline on a single discovery call, connect included.
///
/// The refresh path runs in a spawned task that holds the single-flight lock for as long as it
/// lives, so a Keycloak that accepts the connection and then never answers would keep that lock
/// forever and no later key rotation would ever be picked up again. Deliberately generous rather
/// than tight: `KeycloakConfig::refresh_timeout` is what bounds the *request* a caller waits in,
/// while this only guarantees the task behind it ends at all.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of an error response is quoted back. Keycloak's own error bodies are one short JSON
/// object; anything longer is a proxy's HTML page, which adds nothing the status code does not.
const MAX_QUOTED_BODY: usize = 200;

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// The client every discovery call shares.
///
/// Built once: a `reqwest::Client` owns a connection pool, and constructing a fresh one per call
/// threw that away on every request.
fn discovery_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(DISCOVERY_TIMEOUT)
            .build()
            .unwrap_or_else(|err| {
                // Only fails if the TLS backend cannot be initialised, where the default client
                // would fail the same way. A client without a deadline still beats no discovery.
                warn!(?err, "Could not build the discovery HTTP client with a timeout. Falling back to the default client.");
                reqwest::Client::new()
            })
    })
}

/// Turns a non-2xx answer into an error instead of feeding it to the JSON parser.
///
/// Without this every failure mode of the server — a 404 for a misspelled realm, a 503 while
/// Keycloak is still starting, a proxy's HTML error page — surfaced as "could not decode payload",
/// because `Response::json` does not look at the status. That is the message an operator gets when
/// startup aborts, so it has to name the actual cause.
async fn require_document(response: Response) -> Result<Response, RequestError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    // Read only on the error path, and only to be quoted: a wrong realm is answered with
    // `{"error":"Realm does not exist"}`, which is the diagnosis itself.
    let body = response.text().await.unwrap_or_default();
    let body: String = body.trim().chars().take(MAX_QUOTED_BODY).collect();

    Err(RequestError::Status {
        status,
        body: if body.is_empty() {
            String::from("<empty body>")
        } else {
            body
        },
    })
}

/// Fetches the realm's `.well-known/openid-configuration` document.
pub async fn retrieve_oidc_config(
    discovery_endpoint: impl IntoUrl,
) -> Result<OidcConfig, RequestError> {
    let response = discovery_client()
        .get(discovery_endpoint)
        .send()
        .await
        .map_err(RequestError::send)?;

    require_document(response)
        .await?
        .json::<OidcConfig>()
        .await
        .map_err(RequestError::decode)
}

/// Fetches the JWK set, parsing each key on its own so one unusable entry does not discard the
/// whole set.
pub async fn retrieve_jwk_set(
    jwk_set_endpoint: impl IntoUrl,
) -> Result<jsonwebtoken::jwk::JwkSet, RequestError> {
    #[derive(Deserialize)]
    pub struct RawJwkSet {
        pub keys: Vec<serde_json::Value>,
    }
    let response = discovery_client()
        .get(jwk_set_endpoint)
        .send()
        .await
        .map_err(RequestError::send)?;

    let raw_set = require_document(response)
        .await?
        .json::<RawJwkSet>()
        .await
        .map_err(RequestError::decode)?;
    let mut set = jsonwebtoken::jwk::JwkSet { keys: Vec::new() };
    for key in raw_set.keys {
        match serde_json::from_value::<jsonwebtoken::jwk::Jwk>(key) {
            Ok(parsed) => set.keys.push(parsed),
            Err(err) => tracing::warn!(?err, "Found non-decodable JWK"),
        }
    }
    Ok(set)
}
