//! OIDC discovery and the cached verification keys every request is validated against.
//!
//! A `KeycloakAuthInstance` performs its first discovery while it is being built, and fails
//! construction if that does not succeed — a process that cannot verify a single token is more
//! useful dead, where an orchestrator will restart it, than alive serving blanket 503s.
//!
//! Afterwards the key set refreshes purely on demand: no timer, no background task, and no traffic
//! towards Keycloak while nothing is rotating. A request whose token names an unknown `kid`
//! triggers one re-discovery and retries within that same request, bounded by
//! `KeycloakConfig::refresh_timeout` so a slow Keycloak cannot stall it, and rate-limited by
//! `KeycloakConfig::min_refresh_interval` so a flood of fabricated `kid`s cannot become a request
//! storm. A refresh that fails leaves the previously discovered keys in place.

use educe::Educe;
use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet, PublicKeyUse};
use std::future::Future;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLockReadGuard;
use tokio::time::timeout;
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

    /// Retry strategy for the *initial* discovery: (maximum attempts, delay in seconds).
    ///
    /// `maximum attempts` counts the first try, so `(5, 1)` means five requests one second apart,
    /// not five retries on top of an initial one.
    ///
    /// Generous by default, because failing here aborts startup: in a compose stack Keycloak is
    /// routinely slower to become ready than ISM is, and crash-looping through that is noise. It
    /// only delays the abort — a wrong realm or an unreachable host still brings the process down,
    /// just ~90s later.
    #[builder(default = (18, 5))]
    pub startup_retry: (usize, u64),

    /// Retry strategy for an on-demand refresh, in the same shape as `startup_retry`.
    ///
    /// One attempt, because this runs inside a request. A reachable Keycloak answers in
    /// milliseconds; an unreachable one must not be waited on while a caller holds a connection
    /// open. If the single attempt fails the cached keys still stand and the request gets a 401.
    #[builder(default = (1, 0))]
    pub refresh_retry: (usize, u64),

    /// Hard ceiling on how long a request may be held while an on-demand refresh runs.
    ///
    /// Without it, `refresh_retry` bounds the number of attempts but not the time each one takes,
    /// and a Keycloak that accepts connections without answering would hang the request for as
    /// long as the HTTP client allows.
    #[builder(default = std::time::Duration::from_secs(2))]
    pub refresh_timeout: Duration,

    /// Minimum time between two OIDC discoveries.
    ///
    /// The JWKS is refreshed on demand, when a token names a `kid` the cached key set does not
    /// contain. Since that trigger is caller-controlled, this cooldown is what keeps a flood of
    /// fabricated `kid`s from turning into a request storm against Keycloak.
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
    /// Holds the last *successful* discovery. A failed refresh leaves the previous one in place,
    /// so this is `Some` from construction onwards — `KeycloakAuthInstance::new` does not return
    /// until one discovery has succeeded.
    pub discovery: Action<OidcDiscoveryEndpoint, DiscoveredData>,
    /// Monotonic base for `last_refresh_ms`.
    started_at: Instant,
    /// Millis since `started_at` at which the last discovery was started. Drives the cooldown that
    /// keeps a flood of unknown `kid`s from hammering Keycloak.
    ///
    /// Starts at `0` against a `started_at` stamped once the initial discovery has succeeded, so
    /// the cooldown applies from startup — nothing can have rotated in the moment since.
    ///
    /// An atomic rather than a lock: this is read on *every* authenticated request, and a relaxed
    /// load neither suspends nor bounces a cache line between cores the way even an uncontended
    /// mutex acquire does. The compare-exchange in `refresh_for_request` also expresses the
    /// "exactly one caller starts the refresh" rule more directly than holding a lock across the
    /// check and the stamp.
    last_refresh_ms: AtomicU64,
}

impl KeycloakAuthInstance {
    /// Creates a new `KeycloakAuthInstance`, running the initial OIDC discovery to completion
    /// before returning.
    ///
    /// Deliberately fallible and deliberately awaited here rather than dispatched into a task:
    /// without discovered keys not one token can be verified, so a process that starts anyway only
    /// serves blanket 503s, forever, since nothing re-runs discovery on a timer. Returning `Err` —
    /// which the caller turns into a startup abort — surfaces a bad realm or an unreachable
    /// Keycloak where it can actually be seen and restarted.
    ///
    /// Afterwards the key set is refreshed purely on demand: a token naming a `kid` outside the
    /// cached set makes `decode_and_validate` re-run discovery and retry within the same request.
    /// Signing-key rotation therefore costs added latency on exactly one request and is never
    /// visible to a client — no timer, no background task, and no traffic towards Keycloak while
    /// nothing is rotating.
    pub async fn new(kc_config: KeycloakConfig) -> Result<Self, AuthError> {
        let id = uuid::Uuid::now_v7();
        let oidc_discovery_endpoint = OidcDiscoveryEndpoint::from_server_and_realm(
            kc_config.server.clone(),
            &kc_config.realm,
        );

        // The initial run, on the generous startup budget. Its result is installed directly, so a
        // failure is returned to the caller instead of being buried in a spawned task.
        let discovered = perform_oidc_discovery(
            oidc_discovery_endpoint.clone(),
            kc_config.startup_retry.0,
            Duration::from_secs(kc_config.startup_retry.1),
        )
        .await?;

        let discovery = Action::new(move |oidc_discovery_endpoint: &OidcDiscoveryEndpoint| {
            let oidc_discovery_endpoint = oidc_discovery_endpoint.clone();

            async move {
                perform_oidc_discovery(
                    oidc_discovery_endpoint,
                    kc_config.refresh_retry.0,
                    Duration::from_secs(kc_config.refresh_retry.1),
                )
                .await
                .ok()
            }
        });

        discovery.seed(discovered).await;

        Ok(Self {
            id,
            config: kc_config,
            oidc_discovery_endpoint,
            discovery,
            // Discovery was just dispatched above, so the key set counts as fresh from now on.
            started_at: Instant::now(),
            last_refresh_ms: AtomicU64::new(0),
        })
    }

    /// Milliseconds elapsed since this instance was created.
    fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    /// Refreshes the OIDC configuration and JWKS, subject to the configured cooldown.
    ///
    /// Callers may invoke this per failing request; the cooldown is what keeps that safe.
    pub async fn refresh_for_request(&self) {
        // Created *before* testing `is_pending`, and that order is the point. `notify_waiters`
        // stores no permit and wakes only waiters that already exist when it fires; a `Notified`
        // counts as registered from the moment it is created, not when it is first polled.
        // Reversed, a discovery completing in the gap between the check and this line would leave
        // the caller waiting for the *next* discovery — and nothing here runs on a timer, so there
        // may not be one.
        let notified = self.discovery.notified();

        // Wait for an ongoing discovery rather than starting a competing one. The timeout is a
        // second, independent guard: it bounds this wait no matter what happens to that discovery.
        if self.discovery.is_pending() {
            if timeout(self.config.refresh_timeout, notified)
                .await
                .is_err()
            {
                debug!("Gave up waiting for an in-flight OIDC discovery.");
            }
            return;
        }

        let now = self.now_ms();
        let last = self.last_refresh_ms.load(Ordering::Acquire);
        if now.saturating_sub(last) < self.config.min_refresh_interval.as_millis() as u64 {
            debug!("Skipping OIDC discovery: still within the refresh cooldown.");
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

        let dispatched = self
            .discovery
            .dispatch(self.oidc_discovery_endpoint.clone());

        match timeout(self.config.refresh_timeout, dispatched).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                // Happens when the runtime is shutting down and the task is aborted. The next
                // request will retry; there is nothing to recover here.
                warn!(error = %err, "OIDC discovery task did not complete.");
            }
            Err(_) => {
                // The discovery keeps running in the background and will install its result if it
                // succeeds. We simply stop holding the request open for it.
                debug!(
                    timeout = ?self.config.refresh_timeout,
                    "OIDC discovery outlasted the request budget. Continuing with the cached keys."
                );
            }
        }
    }

    /// A read guard over the most recent discovery result.
    pub async fn discovered(&self) -> Discovered<'_> {
        Discovered {
            // Note: Tokio's RwLock implementation prioritizes write access to prevent starvation. This is fine and will not block writes.
            lock: self.discovery.value().await,
        }
    }

    /// Builds an instance without contacting Keycloak, for tests that only need the surrounding
    /// wiring rather than a working key set.
    ///
    /// The key set stays empty, so every token validated through it is rejected — which is why
    /// this is test-only: it is precisely the state `new` exists to make unreachable.
    #[cfg(test)]
    pub(crate) fn without_discovery(kc_config: KeycloakConfig) -> Self {
        let oidc_discovery_endpoint = OidcDiscoveryEndpoint::from_server_and_realm(
            kc_config.server.clone(),
            &kc_config.realm,
        );

        Self {
            id: uuid::Uuid::now_v7(),
            config: kc_config,
            oidc_discovery_endpoint,
            discovery: Action::new(|_: &OidcDiscoveryEndpoint| async move { None }),
            started_at: Instant::now(),
            last_refresh_ms: AtomicU64::new(0),
        }
    }
}

/// Borrowed view onto the cached discovery result, held for the duration of one validation.
pub struct Discovered<'a> {
    lock: RwLockReadGuard<'a, Option<DiscoveredData>>,
}

impl Discovered<'_> {
    /// The issuer Keycloak advertises in its discovery document.
    ///
    /// Only `None` if the cached data were somehow never installed, which construction rules out.
    pub fn issuer(&self) -> Option<&str> {
        self.lock
            .as_ref()
            .map(|d| d.oidc_config.standard_claims.issuer.as_str())
    }

    /// The keys installed by the last successful discovery. Pass them to `keys_for_kid`.
    pub fn decoding_keys(&self) -> &[JwkDecodingKey] {
        self.lock.as_ref().map_or(&[], |d| &d.decoding_keys)
    }
}

/// The keys published under `kid`, or `None` when none of them carries it.
///
/// A miss is the single failure shape a signing-key rotation can produce, and it is what
/// `decode_and_validate` gates rediscovery on — the signature check itself cannot tell a rotated
/// key from a forged token, so the decision has to be made here, before any crypto.
///
/// Deliberately no fall back to the whole key set on a miss: that both destroyed the signal (every
/// unknown `kid` then failed the same way a tampered token does) and turned each such request into
/// N public-key operations.
///
/// A free function over the key slice rather than a method on `Discovered`, so it can be tested
/// against a hand-built key set instead of a live discovery.
pub fn keys_for_kid<'a>(keys: &'a [JwkDecodingKey], kid: &str) -> Option<Vec<&'a DecodingKey>> {
    let matching: Vec<_> = keys
        .iter()
        .filter(|it| it.kid.as_deref() == Some(kid))
        .map(|it| &it.key)
        .collect();

    (!matching.is_empty()).then_some(matching)
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
                warn!(
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
    // A successful run is routine and stays at DEBUG. Failures below remain at ERROR: discovery
    // only ever runs at startup or on a suspected rotation, so a failure always means either the
    // process is about to abort or a rotation went unpicked-up.
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
        if !matches!(jwk.common.public_key_use,Some(PublicKeyUse::Signature) | None) {
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
                decoding_keys.push(JwkDecodingKey { kid: jwk.common.key_id.clone(), key, });
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
        warn!("No public key for signature verification available. Every token verification will fail.");
    } else {
        info!(keys = ?usable_keys,"Public key(s) for signature verification available.");
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
