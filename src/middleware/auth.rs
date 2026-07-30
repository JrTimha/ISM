//! Wiring ISM's configuration into the Keycloak auth layer.
//!
//! Only the wiring lives here: reading `[token_issuer]` out of ISM's own config and deciding that
//! a failed discovery is fatal. The validation itself is `crate::auth`, which is meant to be
//! extractable as a standalone crate — so anything that knows about `ISMConfig`, or that decides
//! how *this* application reacts to a broken realm, has to sit on this side of that line.

use crate::auth::{
    AppRole, KeycloakAuthInstance, KeycloakAuthLayer, KeycloakConfig, error_chain,
};
use crate::core::TokenIssuer;
use std::time::Duration;
use url::Url;

/// Builds the auth middleware, performing the initial OIDC discovery on the way.
///
/// Panics if that discovery does not succeed, in line with the other startup-configuration
/// failures here and in `main`. Without discovered keys not a single token can be verified and
/// nothing re-runs discovery on a timer, so the alternative is a process that answers every
/// authenticated request with a 503 for as long as it stays up.
pub async fn auth_layer(config: TokenIssuer) -> KeycloakAuthLayer<AppRole> {
    let server = Url::parse(&config.iss_host).expect("Invalid Keycloak Host");

    // Runs the initial discovery and, from the issuer it reports, builds the `ValidationPolicy`
    // the instance then carries. A bad `[token_issuer]` section fails here too, as
    // `AuthError::InvalidValidationPolicy`.
    let keycloak_auth_instance = KeycloakAuthInstance::new(
        KeycloakConfig::builder()
            .server(server)
            .realm(config.iss_realm)
            .expected_audiences(config.expected_audiences)
            .expected_azp(config.expected_azp)
            .allowed_algorithms(config.allowed_algorithms)
            .min_refresh_interval(Duration::from_secs(config.jwks_min_refresh_interval_secs))
            .build(),
    )
    .await
    .unwrap_or_else(|err| {
        panic!("Auth setup failed, refusing to start: {}", error_chain(&err))
    });
    KeycloakAuthLayer::<AppRole>::builder().instance(keycloak_auth_instance).build()
}
