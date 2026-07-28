//! OIDC discovery and the cached verification keys every request is validated against.
//!
//! A `KeycloakAuthInstance` starts discovery when it is built and afterwards refreshes purely on
//! demand — a token that fails to verify triggers one re-discovery, rate-limited by
//! `KeycloakConfig::min_refresh_interval`.

use educe::Educe;
use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet, PublicKeyUse};
use std::future::Future;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLockReadGuard;
use tracing::{debug, error, info, warn};
use typed_builder::TypedBuilder;
use url::Url;

use crate::auth::{
    action::Action,
    error::{AuthError, error_chain},
    oidc::OidcConfig,
    oidc_discovery,
};
/// The realm's `.well-known/openid-configuration` URL.
#[derive(Debug, Clone)]
pub struct OidcDiscoveryEndpoint(pub Url);

impl OidcDiscoveryEndpoint {
    pub fn from_server_and_realm(server: Url, realm: &str) -> Self {
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

    /// The retry strategy to be used: (maximum attempts, delay in seconds).
    ///
    /// `maximum attempts` counts the first try, so `(5, 1)` means five requests one second apart,
    /// not five retries on top of an initial one.
    #[builder(default = (5, 5))]
    pub retry: (usize, u64),

    /// Minimum time between two OIDC discoveries.
    ///
    /// The JWKS is refreshed on demand, when a token fails to verify against the cached keys.
    /// Since that trigger is caller-controlled, this cooldown is what keeps a flood of
    /// unverifiable tokens from turning into a request storm against Keycloak.
    #[builder(default = std::time::Duration::from_secs(30))]
    pub min_refresh_interval: Duration,
}

/// A verification key together with the `kid` it was published under, so incoming tokens can be
/// matched to a single key instead of being tried against every key in the set.
pub struct JwkDecodingKey {
    pub kid: Option<String>,
    pub key: DecodingKey,
}

fn debug_decoding_keys(
    decoding_keys: &[JwkDecodingKey],
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    f.write_fmt(format_args!("len: {}", decoding_keys.len()))
}

/// One successful discovery: the raw documents plus the keys parsed out of them.
#[derive(Educe)]
#[educe(Debug)]
pub struct DiscoveredData {
    pub oidc_config: OidcConfig,
    #[allow(dead_code)]
    pub jwk_set: JwkSet,
    #[educe(Debug(method(debug_decoding_keys)))]
    pub decoding_keys: Vec<JwkDecodingKey>,
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
    pub id: uuid::Uuid,
    pub config: KeycloakConfig,
    pub oidc_discovery_endpoint: OidcDiscoveryEndpoint,
    pub discovery: Action<OidcDiscoveryEndpoint, Result<DiscoveredData, AuthError>>,
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
    /// Creates a new KeycloakAuthInstance. This immediately starts an initial OIDC discovery
    /// process; until it completes, `KeycloakAuthService::poll_ready` holds requests back.
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
                    Duration::from_secs(kc_config.retry.1),
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
    pub async fn perform_oidc_discovery(&self) {
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

    /// A read guard over the most recent discovery result.
    pub async fn discovered(&self) -> Discovered<'_> {
        Discovered {
            // Note: Tokio's RwLock implementation prioritizes write access to prevent starvation. This is fine and will not block writes.
            lock: self.discovery.value().await,
        }
    }
}

/// Borrowed view onto the cached discovery result, held for the duration of one validation.
pub struct Discovered<'a> {
    lock: RwLockReadGuard<'a, Option<Result<DiscoveredData, AuthError>>>,
}

impl Discovered<'_> {
    fn data(&self) -> Option<&DiscoveredData> {
        self.lock.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The issuer Keycloak advertises in its discovery document. `None` until discovery succeeds.
    pub fn issuer(&self) -> Option<&str> {
        self.data()
            .map(|d| d.oidc_config.standard_claims.issuer.as_str())
    }

    /// Keys worth trying for a token carrying `kid`.
    ///
    /// Matching on `kid` keeps an invalid token to a single signature verification. Trying every
    /// key in the set instead turns each unauthenticated request into N public-key operations.
    /// Falls back to the full set when the token names no `kid`, or names one we do not know —
    /// the latter is what a just-rotated key looks like, and the caller re-discovers on failure.
    pub fn candidate_keys(&self, kid: Option<&str>) -> Vec<&DecodingKey> {
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

/// Runs `op` until it succeeds, at most `max_attempts` times, sleeping `delay` in between.
///
/// Every error is retried, without inspecting it: the only caller talks to Keycloak during
/// discovery, where a misconfiguration and a dropped connection are equally worth another attempt.
/// Intermediate failures are logged at DEBUG; the caller decides what a terminal failure means.
///
/// Spelled with an explicit `Fn() -> Future` bound rather than `AsyncFn`: the latter cannot express
/// that the returned future is `Send`, which the spawned discovery task requires.
async fn retry_fixed<T, E, F, Fut>(max_attempts: usize, delay: Duration, op: F) -> Result<T, E>
where
    E: std::error::Error,
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    // The final attempt is the one after the loop, so it is not followed by a pointless sleep.
    for attempt in 1..max_attempts {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                debug!(
                    attempt,
                    max_attempts,
                    err = error_chain(&err),
                    "Attempt failed. Retrying after delay."
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
    op().await
}

async fn perform_oidc_discovery(
    oidc_discovery_endpoint: OidcDiscoveryEndpoint,
    max_attempts: usize,
    fixed_delay: Duration,
) -> Result<DiscoveredData, AuthError> {
    // Discovery now also runs on a timer, so a successful run is routine and stays at DEBUG.
    // Failures below remain at ERROR, which is what actually needs to be visible.
    debug!("Starting OIDC discovery.");

    // Load OIDC config.
    let oidc_config = retry_fixed(max_attempts, fixed_delay, async || {
        oidc_discovery::retrieve_oidc_config(oidc_discovery_endpoint.0.clone())
            .await
            .map_err(|source| AuthError::OidcDiscovery { source })
    })
    .await
    .inspect_err(|err| {
        error!(err = error_chain(err), "Could not retrieve OIDC config.");
    })?;

    // Parse JWK endpoint if OIDC config is available.
    let jwk_set_endpoint = Url::parse(&oidc_config.standard_claims.jwks_uri)
        .map_err(|source| AuthError::JwkEndpoint { source })
        .inspect_err(|err| {
            error!(
                err = error_chain(err),
                "Could not retrieve jwk_set_endpoint_url."
            );
        })?;

    // Load JWK set if endpoint was parsable.
    let jwk_set = retry_fixed(max_attempts, fixed_delay, async || {
        oidc_discovery::retrieve_jwk_set(jwk_set_endpoint.clone())
            .await
            .map_err(|source| AuthError::JwkSetDiscovery { source })
    })
    .await
    .inspect_err(|err| {
        error!(err = error_chain(err), "Could not retrieve jwk_set.");
    })?;

    debug!(num_keys = jwk_set.keys.len(), "Received new jwk_set.");

    // Create DecodingKey instances from received JWKs.
    let decoding_keys = parse_jwks(&jwk_set);

    Ok(DiscoveredData {
        oidc_config,
        jwk_set,
        decoding_keys,
    })
}

fn parse_jwks(jwk_set: &JwkSet) -> Vec<JwkDecodingKey> {
    let mut decoding_keys = Vec::with_capacity(jwk_set.keys.len());
    let mut usable_keys = Vec::with_capacity(jwk_set.keys.len());

    for jwk in &jwk_set.keys {
        // Keycloak publishes its RSA-OAEP encryption key in the same set. Loading it as a
        // verification key is pointless and widens what a token can be verified against.
        if !matches!(
            jwk.common.public_key_use,
            Some(PublicKeyUse::Signature) | None
        ) {
            continue;
        }

        match DecodingKey::from_jwk(jwk) {
            Ok(key) => {
                usable_keys.push(format!(
                    "kid={} kty={} alg={:?}",
                    jwk.common.key_id.as_deref().unwrap_or("<none>"),
                    key_type_name(&jwk.algorithm),
                    jwk.common.key_algorithm,
                ));
                decoding_keys.push(JwkDecodingKey {
                    kid: jwk.common.key_id.clone(),
                    key,
                });
            }
            Err(err) => {
                error!(
                    ?err,
                    kid = ?jwk.common.key_id,
                    "Received JWK from Keycloak which could not be parsed as a DecodingKey. Ignoring the JWK."
                );
            }
        }
    }

    if decoding_keys.is_empty() {
        warn!(
            "No public key for signature verification available. Every token verification will fail."
        );
    } else {
        info!(
            keys = ?usable_keys,
            "Public key(s) for signature verification available."
        );
    }

    decoding_keys
}

/// Short `kty` label for a JWK, so key details can be logged without dumping the key material.
fn key_type_name(algorithm: &AlgorithmParameters) -> &'static str {
    match algorithm {
        AlgorithmParameters::EllipticCurve(_) => "EC",
        AlgorithmParameters::RSA(_) => "RSA",
        AlgorithmParameters::OctetKey(_) => "oct",
        AlgorithmParameters::OctetKeyPair(_) => "OKP",
        _ => "unknown",
    }
}
