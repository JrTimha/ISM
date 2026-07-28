//! The HTTP calls that fetch Keycloak's discovery document and JWK set.
//!
//! The documents themselves are modelled in `oidc.rs`; this module only does the fetching.

use std::sync::Arc;

use crate::auth::oidc::OidcConfig;
use reqwest::IntoUrl;
use serde::Deserialize;
use snafu::{ResultExt, Snafu};

/// A discovery request that never produced a usable document.
#[derive(Debug, Clone, Snafu)]
#[snafu(visibility(pub))]
pub enum RequestError {
    #[snafu(display("RequestError: Could not send request"))]
    Send { source: Arc<reqwest::Error> },

    #[snafu(display("RequestError: Could not decode payload"))]
    Decode { source: Arc<reqwest::Error> },
}

/// Fetches the realm's `.well-known/openid-configuration` document.
pub async fn retrieve_oidc_config(
    discovery_endpoint: impl IntoUrl,
) -> Result<OidcConfig, RequestError> {
    reqwest::Client::new()
        .get(discovery_endpoint)
        .send()
        .await
        .map_err(Arc::new)
        .context(SendSnafu {})?
        .json::<OidcConfig>()
        .await
        .map_err(Arc::new)
        .context(DecodeSnafu {})
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
    let raw_set = reqwest::Client::new()
        .get(jwk_set_endpoint)
        .send()
        .await
        .map_err(Arc::new)
        .context(SendSnafu {})?
        .json::<RawJwkSet>()
        .await
        .map_err(Arc::new)
        .context(DecodeSnafu {})?;
    let mut set = jsonwebtoken::jwk::JwkSet { keys: Vec::new() };
    for key in raw_set.keys {
        match serde_json::from_value::<jsonwebtoken::jwk::Jwk>(key) {
            Ok(parsed) => set.keys.push(parsed),
            Err(err) => tracing::warn!(?err, "Found non-decodable JWK"),
        }
    }
    Ok(set)
}
