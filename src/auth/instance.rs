use educe::Educe;
use snafu::ResultExt;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::RwLockReadGuard;
use try_again::{StdDuration, delay, retry_async};
use typed_builder::TypedBuilder;
use url::Url;

use crate::auth::{
    action::Action,
    error::{AuthError, JwkEndpointSnafu, JwkSetDiscoverySnafu, OidcDiscoverySnafu},
    oidc::OidcConfig,
    oidc_discovery,
};
#[derive(Debug, Clone)]
pub(crate) struct OidcDiscoveryEndpoint(pub(crate) Url);

impl OidcDiscoveryEndpoint {
    pub(crate) fn from_server_and_realm(server: Url, realm: &str) -> Self {
        let mut url = server;
        url.path_segments_mut()
            .expect("URL not to be a 'cannot-be-a-base' URL. We have to append segments.")
            .extend(&["realms", realm, ".well-known", "openid-configuration"]);
        Self(url)
    }
}

impl Deref for OidcDiscoveryEndpoint {
    type Target = Url;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, TypedBuilder)]
pub struct KeycloakConfig {
    /// Base URL of your Keycloak server. For example: `Url::parse("https://localhost:8443/").unwrap()`.
    pub server: Url,

    /// The realm of you Keycloak server.
    pub realm: String,

    /// The retry strategy to be used: (maximum tries, delay in seconds).
    #[builder(default = (5, 1))]
    pub retry: (usize, u64),

    /// Minimum time between two OIDC discoveries.
    ///
    /// The JWKS is refreshed on demand, when a token fails to verify against the cached keys.
    /// Since that trigger is caller-controlled, this cooldown is what keeps a flood of
    /// unverifiable tokens from turning into a request storm against Keycloak.
    #[builder(default = std::time::Duration::from_secs(30))]
    pub min_refresh_interval: std::time::Duration,
}

/// A verification key together with the `kid` it was published under, so incoming tokens can be
/// matched to a single key instead of being tried against every key in the set.
pub(crate) struct JwkDecodingKey {
    pub(crate) kid: Option<String>,
    pub(crate) key: jsonwebtoken::DecodingKey,
}

fn debug_decoding_keys(
    decoding_keys: &[JwkDecodingKey],
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    f.write_fmt(format_args!("len: {}", decoding_keys.len()))
}

#[derive(TypedBuilder, Educe)]
#[educe(Debug)]
pub(crate) struct DiscoveredData {
    pub(crate) oidc_config: OidcConfig,
    #[allow(dead_code)]
    pub(crate) jwk_set: jsonwebtoken::jwk::JwkSet,
    #[educe(Debug(method(debug_decoding_keys)))]
    pub(crate) decoding_keys: Vec<JwkDecodingKey>,
}

/// The KeycloakAuthInstance is responsible for performing OIDC discovery
/// and will hold onto the retrieved OIDC configuration, including the decoding keys
/// used to decode incoming JWTs.
///
/// You may want to create only a single insatnce of this struct
/// to limit the amount of requests made towards your Keycloak server.
#[derive(Debug)]
pub struct KeycloakAuthInstance {
    #[allow(dead_code)]
    pub(crate) id: uuid::Uuid,
    pub(crate) config: KeycloakConfig,
    pub(crate) oidc_discovery_endpoint: OidcDiscoveryEndpoint,
    pub(crate) discovery: Action<OidcDiscoveryEndpoint, Result<DiscoveredData, AuthError>>,
    /// Monotonic base for `last_refresh_ms`.
    started_at: Instant,
    /// Millis since `started_at` at which the last discovery was started. Drives both the staleness
    /// check and the cooldown that keeps unverifiable tokens from hammering Keycloak.
    ///
    /// An atomic rather than a lock: this is read on *every* authenticated request, and a relaxed
    /// load neither suspends nor bounces a cache line between cores the way even an uncontended
    /// mutex acquire does. The compare-exchange in `perform_oidc_discovery` also expresses the
    /// "exactly one caller starts the refresh" rule more directly than holding a lock across the
    /// check and the stamp.
    last_refresh_ms: AtomicU64,
}

impl KeycloakAuthInstance {
    /// Creates a new KeycloakAuthInstance. This immediately starts an initial OIDC discovery process.
    /// The `is_operational` method will tell you if discovery has taken place.
    /// This may be useful in determining service health.
    ///
    /// Afterwards the key set is refreshed purely on demand: when a token cannot be verified
    /// against the cached keys, `decode_and_validate` re-runs discovery and retries the decode
    /// within the same request. Signing-key rotation therefore costs added latency on exactly one
    /// request and is never visible to a client — no timer, no background task, and no traffic
    /// towards Keycloak while nothing is rotating.
    pub fn new(kc_config: KeycloakConfig) -> Self {
        let id = uuid::Uuid::now_v7();
        let oidc_discovery_endpoint = OidcDiscoveryEndpoint::from_server_and_realm(
            kc_config.server.clone(),
            &kc_config.realm,
        );

        let discovery = Action::new(move |oidc_discovery_endpoint: &OidcDiscoveryEndpoint| {
            let oidc_discovery_endpoint = oidc_discovery_endpoint.clone();

            async move {
                perform_oidc_discovery(
                    oidc_discovery_endpoint,
                    kc_config.retry.0,
                    std::time::Duration::from_secs(kc_config.retry.1),
                )
                .await
            }
        });

        discovery.dispatch(oidc_discovery_endpoint.clone());

        Self {
            id,
            config: kc_config,
            oidc_discovery_endpoint,
            discovery,
            // Discovery was just dispatched above, so the key set counts as fresh from now on.
            started_at: Instant::now(),
            last_refresh_ms: AtomicU64::new(0),
        }
    }

    /// Milliseconds elapsed since this instance was created.
    fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    /// Refreshes the OIDC configuration and JWKS, subject to the configured cooldown.
    ///
    /// Callers may invoke this per failing request; the cooldown is what keeps that safe.
    pub(crate) async fn perform_oidc_discovery(&self) {
        // Wait for an ongoing discovery rather than starting a competing one.
        if self.discovery.is_pending() {
            self.discovery.notified().await;
            return;
        }

        let now = self.now_ms();
        let last = self.last_refresh_ms.load(Ordering::Acquire);
        if now.saturating_sub(last) < self.config.min_refresh_interval.as_millis() as u64 {
            tracing::trace!("Skipping OIDC discovery: still within the refresh cooldown.");
            return;
        }

        // Claim the refresh before dispatching. Exactly one of any number of concurrent callers
        // wins the exchange and starts discovery; the rest return and keep serving the current
        // keys. This also covers callers arriving before the dispatched task flips `pending`.
        if self
            .last_refresh_ms
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        if let Err(err) = self
            .discovery
            .dispatch(self.oidc_discovery_endpoint.clone())
            .await
        {
            // Happens when the runtime is shutting down and the task is aborted. The next request
            // will retry; there is nothing to recover here.
            tracing::warn!(error = %err, "OIDC discovery task did not complete.");
        }
    }

    /// Returns true after a successful OIDC discovery.
    pub async fn is_operational(&self) -> bool {
        self.discovery
            .value()
            .await
            .as_ref()
            .is_some_and(|it| it.is_ok())
    }

    pub(crate) async fn discovered(&self) -> Discovered<'_> {
        Discovered {
            // Note: Tokio's RwLock implementation prioritizes write access to prevent starvation. This is fine and will not block writes.
            lock: self.discovery.value().await,
        }
    }
}

pub(crate) struct Discovered<'a> {
    lock: RwLockReadGuard<'a, Option<Result<DiscoveredData, AuthError>>>,
}

impl Discovered<'_> {
    fn data(&self) -> Option<&DiscoveredData> {
        self.lock.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The issuer Keycloak advertises in its discovery document. `None` until discovery succeeds.
    pub(crate) fn issuer(&self) -> Option<&str> {
        self.data()
            .map(|d| d.oidc_config.standard_claims.issuer.as_str())
    }

    /// Keys worth trying for a token carrying `kid`.
    ///
    /// Matching on `kid` keeps an invalid token to a single signature verification. Trying every
    /// key in the set instead turns each unauthenticated request into N public-key operations.
    /// Falls back to the full set when the token names no `kid`, or names one we do not know —
    /// the latter is what a just-rotated key looks like, and the caller re-discovers on failure.
    pub(crate) fn candidate_keys(&self, kid: Option<&str>) -> Vec<&jsonwebtoken::DecodingKey> {
        let Some(keys) = self.data().map(|d| d.decoding_keys.as_slice()) else {
            return Vec::new();
        };

        if let Some(kid) = kid {
            let matching: Vec<_> = keys
                .iter()
                .filter(|it| it.kid.as_deref() == Some(kid))
                .map(|it| &it.key)
                .collect();
            if !matching.is_empty() {
                return matching;
            }
        }

        keys.iter().map(|it| &it.key).collect()
    }
}

async fn perform_oidc_discovery(
    oidc_discovery_endpoint: OidcDiscoveryEndpoint,
    num_retries: usize,
    fixed_delay: StdDuration,
) -> Result<DiscoveredData, AuthError> {
    // Discovery now also runs on a timer, so a successful run is routine and stays at DEBUG.
    // Failures below remain at ERROR, which is what actually needs to be visible.
    tracing::debug!("Starting OIDC discovery.");

    // Load OIDC config.
    let oidc_config = retry_async(async move || {
        oidc_discovery::retrieve_oidc_config(oidc_discovery_endpoint.0.clone())
            .await
            .context(OidcDiscoverySnafu {})
    })
    .delayed_by(delay::Fixed::of(fixed_delay).take(num_retries))
    .await
    .inspect_err(|err| {
        tracing::error!(
            err = snafu::Report::from_error(err.clone()).to_string(),
            "Could not retrieve OIDC config."
        );
    })?;

    // Parse JWK endpoint if OIDC config is available.
    let jwk_set_endpoint = Url::parse(&oidc_config.standard_claims.jwks_uri)
        .context(JwkEndpointSnafu {})
        .inspect_err(|err| {
            tracing::error!(
                err = snafu::Report::from_error(err.clone()).to_string(),
                "Could not retrieve jwk_set_endpoint_url."
            );
        })?;

    // Load JWK set if endpoint was parsable.
    let jwk_set = retry_async(async move || {
        oidc_discovery::retrieve_jwk_set(jwk_set_endpoint.clone())
            .await
            .context(JwkSetDiscoverySnafu {})
    })
    .delayed_by(delay::Fixed::of(fixed_delay).take(num_retries))
    .await
    .inspect_err(|err| {
        tracing::error!(
            err = snafu::Report::from_error(err.clone()).to_string(),
            "Could not retrieve jwk_set."
        );
    })?;

    tracing::debug!(num_keys = jwk_set.keys.len(), "Received new jwk_set.");

    // Create DecodingKey instances from received JWKs.
    let decoding_keys = parse_jwks(&jwk_set);

    Ok(DiscoveredData {
        oidc_config,
        jwk_set,
        decoding_keys,
    })
}

fn parse_jwks(jwk_set: &jsonwebtoken::jwk::JwkSet) -> Vec<JwkDecodingKey> {
    jwk_set
        .keys
        .iter()
        .filter(|jwk| {
            // Keycloak publishes its RSA-OAEP encryption key in the same set. Loading it as a
            // verification key is pointless and widens what a token can be verified against.
            match &jwk.common.public_key_use {
                Some(jsonwebtoken::jwk::PublicKeyUse::Signature) | None => true,
                Some(other) => {
                    tracing::debug!(
                        ?other,
                        kid = ?jwk.common.key_id,
                        "Ignoring JWK that is not intended for signature verification."
                    );
                    false
                }
            }
        })
        .filter_map(|jwk| match jsonwebtoken::DecodingKey::from_jwk(jwk) {
            Ok(key) => Some(JwkDecodingKey {
                kid: jwk.common.key_id.clone(),
                key,
            }),
            Err(err) => {
                tracing::error!(
                    ?err,
                    kid = ?jwk.common.key_id,
                    "Received JWK from Keycloak which could not be parsed as a DecodingKey. Ignoring the JWK."
                );
                None
            }
        })
        .collect()
}
