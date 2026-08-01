//! OIDC discovery and the cached verification keys every request is validated against.
//!
//! A `KeycloakAuthInstance` performs its first discovery while it is being built, and fails
//! construction if that does not succeed — a process that cannot verify a single token is more
//! useful dead, where an orchestrator will restart it, than alive serving blanket 503s.
//!
//! Afterwards the key set refreshes purely on demand: no timer, no polling loop, and no traffic
//! towards Keycloak while nothing is rotating. A request whose token names an unknown `kid`
//! triggers one re-discovery and retries within that same request, bounded by
//! `KeycloakConfig::refresh_timeout` so a slow Keycloak cannot stall it, and rate-limited by
//! `KeycloakConfig::min_refresh_interval` so a flood of fabricated `kid`s cannot become a request
//! storm. A refresh that fails leaves the previously discovered keys in place.
//!
//! Two Tokio primitives carry that: a `watch` channel holding the current `DiscoveredData`, and a
//! `Mutex` whose guard is the single-flight token for a running refresh and whose payload is the
//! cooldown stamp. See `refresh_jwks_for_request`.

use educe::Educe;
use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet, PublicKeyUse};
use std::future::Future;
use std::ops::Deref;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, watch};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use typed_builder::TypedBuilder;
use url::Url;

use crate::auth::{
    error::{AuthError, error_chain},
    oidc::OidcConfig,
    policy::ValidationPolicy,
};
use crate::auth::oidc_discovery::{retrieve_jwk_set, retrieve_oidc_config};

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

    /// Accepted `aud` values. Must not be empty — that would disable the audience check.
    pub expected_audiences: Vec<String>,

    /// Accepted `azp` values, i.e. which realm clients this service takes tokens from. Empty
    /// disables the check, which means any client in the realm is accepted.
    pub expected_azp: Vec<String>,

    /// Accepted signature algorithms, by their JWA name (`RS256`, `PS512`, …). All entries must
    /// belong to the same family, and symmetric (`HS*`) ones are refused outright.
    pub allowed_algorithms: Vec<String>,

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

impl DiscoveredData {
    pub fn get_issuer(&self) -> &str {
        self.oidc_config.standard_claims.issuer.as_str()
    }
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

    /// The rules every token is validated against.
    /// Fixed for the life of the instance. A later refresh replaces the keys but never this: the
    /// issuer is a trust anchor, and silently following a change to it is not a decision this
    /// middleware should make on its own.
    policy: ValidationPolicy,

    /// The keys every request is validated against, holding the last *successful* discovery.
    ///
    /// A `watch` rather than a lock because a reader takes an `Arc` clone and then holds nothing:
    /// installing a refreshed key set can never stall a validation that is already running, and no
    /// guard is alive across the signature check. It is also what signals completion to a caller
    /// waiting on someone else's refresh — version-based, so a discovery finishing between
    /// subscribing and awaiting still counts as a change.
    ///
    /// Not an `Option`: a failed refresh leaves the previous value in place, and
    /// `KeycloakAuthInstance::new` does not return until one discovery has succeeded.
    keys: watch::Sender<Arc<DiscoveredData>>,

    /// Single-flight token *and* cooldown stamp: the guard is held for as long as a refresh runs,
    /// its payload is when that refresh started.
    ///
    /// One primitive for both jobs, because they are the same decision. `try_lock_owned` failing
    /// is exactly "a discovery is already in flight" — no separate pending flag that a panicking
    /// discovery could leave stuck set — and the payload behind that same lock is the timestamp
    /// the cooldown is measured from, so the check and the stamp cannot be interleaved by a
    /// competing caller.
    refresh: Arc<Mutex<Instant>>,
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
    /// visible to a client — no timer, no polling loop, and no traffic towards Keycloak while
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

        //generate warnings for unsafe configurations:
        if kc_config.server.scheme() != "https" {
            warn!(
            keycloak_host = %kc_config.server,
            "Keycloak is configured over plaintext HTTP. Tokens and the JWKS are exchanged in the \
             clear — use HTTPS outside of local development."
        );
        }
        if kc_config.expected_audiences.iter().any(|it| it == "account") && kc_config.expected_azp.is_empty() {
            warn!(
                "Accepting the 'account' audience with no 'expected_azp' restriction: Keycloak adds \
                 this audience to every access token of every client in the realm, so any client in \
                 the realm is accepted. Configure an audience mapper and set expected_audiences / \
                 expected_azp."
            );
        }

        // Built here, and only here, because the issuer it pins is the one the realm just
        // advertised — a configured frontend URL makes that differ from anything `server`/`realm`
        // could be used to reconstruct.
        let policy = ValidationPolicy::new(
            discovered.get_issuer().to_owned(),
            kc_config.expected_audiences.clone(),
            kc_config.expected_azp.clone(),
            &kc_config.allowed_algorithms,
        )
        .map_err(|reason| AuthError::InvalidValidationPolicy { reason })?;

        info!(issuer = %policy.expected_issuer, "Pinned the token issuer from OIDC discovery");

        let (keys, _) = watch::channel(Arc::new(discovered));

        Ok(Self {
            id,
            config: kc_config,
            oidc_discovery_endpoint,
            policy,
            keys,
            // Discovery just succeeded, so the cooldown applies from startup on — nothing can have
            // rotated in the moment since.
            refresh: Arc::new(Mutex::new(Instant::now())),
        })
    }

    /// Refreshes the OIDC configuration and JWKS, subject to the configured cooldown.
    /// Callers may invoke this per failing request; the cooldown is what keeps that safe.
    pub async fn refresh_jwks_for_request(&self) {
        // Subscribed first, before anything can complete. The receiver records the version present
        // right now, so a discovery finishing between here and either wait below still registers as
        // a change — unlike a notification primitive, which only reaches waiters that already exist
        // when it fires.
        let mut rx = self.keys.subscribe();

        // A refresh in flight is one that holds this lock. Wait for its result rather than start a
        // competing run; the timeout bounds that wait independently of what becomes of it.
        let Ok(mut last_refresh) = Arc::clone(&self.refresh).try_lock_owned() else {
            if timeout(self.config.refresh_timeout, rx.changed()).await.is_err() {
                debug!("Awaiting the in-flight OIDC discovery too long, timeouting and use old jwks.");
            }
            return;
        };

        if last_refresh.elapsed() < self.config.min_refresh_interval {
            debug!("Skipping OIDC discovery: still within the refresh cooldown.");
            return;
        }
        // Stamped at the start, so the cooldown covers the discovery's own runtime too.
        *last_refresh = Instant::now();

        let keys = self.keys.clone();
        let endpoint = self.oidc_discovery_endpoint.clone();
        let (max_attempts, delay) = self.config.refresh_retry;

        // Spawned rather than awaited inline for two reasons: a client disconnecting mid-request
        // drops this function's future, and the refresh must not be cancelled along with it; and a
        // discovery outlasting the request budget below still gets to install its result.
        tokio::spawn(async move {
            // Moved in so the lock is released when the discovery ends — including when it panics,
            // which is the whole reason the single-flight marker is a guard and not a flag.
            let _guard = last_refresh;

            match perform_oidc_discovery(endpoint, max_attempts, Duration::from_secs(delay)).await {
                Ok(discovered) => {
                    let _ = keys.send_replace(Arc::new(discovered));
                    debug!("Refreshed the jwks.");
                }
                Err(err) => {
                    warn!(err = error_chain(&err), "OIDC discovery failed. Continuing with the cached keys.");
                    // Bumps the version without touching the value: the keys a successful run left
                    // behind must survive a failed one, but the caller waiting below has to be
                    // released now rather than sit out its whole timeout for a result that is
                    // never coming.
                    keys.send_modify(|_| {});
                }
            }
        });

        if timeout(self.config.refresh_timeout, rx.changed()).await.is_err() {
            debug!(
                timeout = ?self.config.refresh_timeout,
                "OIDC discovery outlasted the request budget. Continuing with the cached keys."
            );
        }
    }

    /// The most recent successful discovery.
    ///
    /// Synchronous and cheap — an `Arc` clone — because this runs on every authenticated request.
    /// The caller owns what it gets back and holds no lock on this instance.
    pub fn get_discovered_jwks(&self) -> Arc<DiscoveredData> {
        self.keys.borrow().clone()
    }

    /// The rules every token validated through this instance is held to.
    pub fn get_jwt_validation_policy(&self) -> &ValidationPolicy {
        &self.policy
    }

    /// Builds an instance without contacting Keycloak, for tests that only need the surrounding
    /// wiring rather than a working key set.
    ///
    /// The key set stays empty, so every token validated through it is rejected — which is why
    /// this is test-only: it is precisely the state `new` exists to make unreachable.
    ///
    /// The refresh stamp starts at "now", which puts the instance inside the cooldown from the
    /// moment it exists. A test that does reach the key lookup therefore gets its
    /// `UnknownSigningKey` without any attempt to contact the configured Keycloak.
    ///
    /// The policy is passed in rather than derived, because deriving it needs a discovered issuer
    /// and there is none here.
    #[cfg(test)]
    pub fn without_discovery(kc_config: KeycloakConfig, policy: ValidationPolicy) -> Self {
        let oidc_discovery_endpoint = OidcDiscoveryEndpoint::from_server_and_realm(
            kc_config.server.clone(),
            &kc_config.realm,
        );

        let (keys, _) = watch::channel(Arc::new(DiscoveredData {
            oidc_config: OidcConfig::default(),
            jwk_set: JwkSet { keys: Vec::new() },
            decoding_keys: Vec::new(),
        }));

        Self {
            id: uuid::Uuid::now_v7(),
            config: kc_config,
            oidc_discovery_endpoint,
            policy,
            keys,
            refresh: Arc::new(Mutex::new(Instant::now())),
        }
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



/// A successful run is routine and stays at DEBUG. Failures below remain at ERROR: discovery
/// only ever runs at startup or on a suspected rotation, so a failure always means either the
/// process is about to abort or a rotation went unpicked-up.
async fn perform_oidc_discovery(
    oidc_discovery_endpoint: OidcDiscoveryEndpoint,
    max_attempts: usize,
    fixed_delay: Duration,
) -> Result<DiscoveredData, AuthError> {

    debug!("Starting a new OIDC discovery.");
    // Load OIDC config.
    let oidc_config = retry_fixed(max_attempts, fixed_delay, async || {
        retrieve_oidc_config(oidc_discovery_endpoint.0.clone()).await
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
        retrieve_jwk_set(jwk_set_endpoint.clone()).await
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

                decoding_keys.push(JwkDecodingKey { kid: jwk.common.key_id.clone(), key });
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

/// What a rotation storm does to `refresh_jwks_for_request`: hundreds of requests arriving with the same
/// unknown `kid` while one discovery runs behind them.
///
/// Every test here drives the real `KeycloakAuthInstance` against a stand-in Keycloak on a loopback
/// port, so the `watch` channel, the single-flight lock and the request budget are exercised as
/// they are wired in production — only the network peer is fake, and only because a real one cannot
/// be told to answer slowly or to fail on command.
#[cfg(test)]
mod refresh_tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    use crate::auth::oidc::OpenIDConnectStandardDiscoveryClaims;

    /// A Keycloak that answers as slowly as a test needs it to, and counts who asked.
    #[derive(Clone)]
    struct MockKeycloak {
        /// Requests that *reached* the discovery document, in-flight ones included. The
        /// single-flight assertions are about how many calls Keycloak sees, not how many finish.
        discoveries: Arc<AtomicUsize>,
        /// How long the discovery document takes to answer. Adjustable, so an instance can be
        /// built instantly and only the refresh under test runs slowly.
        latency_ms: Arc<AtomicU64>,
        /// Makes the discovery document fail, for the "cached keys must survive" case.
        fail: Arc<AtomicBool>,
        jwks_uri: String,
    }

    impl MockKeycloak {
        fn set_latency(&self, latency: Duration) {
            self.latency_ms
                .store(latency.as_millis() as u64, Ordering::SeqCst);
        }

        fn discoveries(&self) -> usize {
            self.discoveries.load(Ordering::SeqCst)
        }
    }

    /// Each response carries a distinct issuer, which is how a test tells *which* discovery ended
    /// up installed without needing real key material.
    async fn discovery_document(
        State(mock): State<MockKeycloak>,
    ) -> Result<Json<OidcConfig>, (StatusCode, String)> {
        let nth = mock.discoveries.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::time::sleep(Duration::from_millis(mock.latency_ms.load(Ordering::SeqCst))).await;

        if mock.fail.load(Ordering::SeqCst) {
            // Exactly what Keycloak answers a request for a realm it does not have — the most
            // likely misconfiguration, and the one the error message has to be readable for.
            return Err((
                StatusCode::NOT_FOUND,
                String::from(r#"{"error":"Realm does not exist"}"#),
            ));
        }

        Ok(Json(OidcConfig {
            standard_claims: OpenIDConnectStandardDiscoveryClaims {
                issuer: format!("http://mock.invalid/realms/test-{nth}"),
                jwks_uri: mock.jwks_uri.clone(),
                ..Default::default()
            },
            ..Default::default()
        }))
    }

    /// Deliberately empty: these tests are about the refresh choreography, not about verifying a
    /// signature, and an empty set is a state `parse_jwks` handles without complaint.
    async fn empty_jwk_set() -> Json<serde_json::Value> {
        Json(serde_json::json!({ "keys": [] }))
    }

    /// Makes the instance's own `debug!`/`warn!` events visible, which is the only window into
    /// which branch of `refresh_jwks_for_request` a caller took — the assertions can see the outcome,
    /// not the path.
    ///
    /// Run with `RUST_LOG=ism=debug cargo test --lib auth::instance -- --nocapture`. Silent
    /// otherwise: without `RUST_LOG` the filter drops everything, and without `--nocapture` the
    /// harness swallows the output of a passing test.
    ///
    /// Deliberately global, not `tracing::subscriber::set_default`: that installs a *thread-local*
    /// subscriber, and the discovery these tests are about runs in a spawned task on some other
    /// worker thread, where a thread-local one would not be seen. A global subscriber can only be
    /// installed once per process though, and one process runs every test in this binary — hence
    /// the `Once`, and hence `try_init`, whose `Err` on a second attempt is expected.
    fn init_tracing() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_env_filter("ism=debug")
                // Routes through the harness's capture, so `--nocapture` governs it like any other
                // test output instead of printing unconditionally.
                .with_test_writer()
                .try_init();
        });
    }

    /// Binds a mock realm to an ephemeral loopback port and returns its base URL.
    async fn start_mock() -> (Url, MockKeycloak) {
        init_tracing();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral loopback port to be available");
        let addr = listener
            .local_addr()
            .expect("the bound listener to have an address");

        let mock = MockKeycloak {
            discoveries: Arc::new(AtomicUsize::new(0)),
            latency_ms: Arc::new(AtomicU64::new(0)),
            fail: Arc::new(AtomicBool::new(false)),
            jwks_uri: format!("http://{addr}/jwks"),
        };

        let app = Router::new()
            .route(
                "/realms/{realm}/.well-known/openid-configuration",
                get(discovery_document),
            )
            .route("/jwks", get(empty_jwk_set))
            .with_state(mock.clone());

        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("the mock server to keep running for the test's duration");
        });

        let base = Url::parse(&format!("http://{addr}/")).expect("the mock URL to parse");
        (base, mock)
    }

    fn config(server: Url, refresh_timeout: Duration, cooldown: Duration) -> KeycloakConfig {
        KeycloakConfig::builder()
            .server(server)
            .realm(String::from("test"))
            .expected_audiences(vec![String::from("ism")])
            .expected_azp(vec![String::from("ism-client")])
            .allowed_algorithms(vec![String::from("RS256")])
            // One attempt each: a retry would blur the discovery counts below.
            .startup_retry((1, 0))
            .refresh_retry((1, 0))
            .refresh_timeout(refresh_timeout)
            .min_refresh_interval(cooldown)
            .build()
    }

    async fn instance_for(
        server: Url,
        refresh_timeout: Duration,
        cooldown: Duration,
    ) -> Arc<KeycloakAuthInstance> {
        Arc::new(
            KeycloakAuthInstance::new(config(server, refresh_timeout, cooldown))
                .await
                .expect("the startup discovery against the mock to succeed"),
        )
    }

    /// Fires `callers` concurrent `refresh_jwks_for_request` calls and returns once every one of them
    /// has returned — the point at which every `rx` the function subscribed is dropped.
    async fn burst(instance: &Arc<KeycloakAuthInstance>, callers: usize) -> Duration {
        let started = Instant::now();

        let handles: Vec<_> = (0..callers)
            .map(|_| {
                let instance = Arc::clone(instance);
                tokio::spawn(async move { instance.refresh_jwks_for_request().await })
            })
            .collect();

        for handle in handles {
            handle.await.expect("no caller to panic");
        }

        started.elapsed()
    }

    /// Waits for a refresh to be installed, i.e. for the issuer to move off `previous`.
    async fn await_installed_issuer(
        instance: &KeycloakAuthInstance,
        previous: &str,
        within: Duration,
    ) -> String {
        let deadline = Instant::now() + within;
        loop {
            let current = instance.get_discovered_jwks().get_issuer().to_owned();
            if current != previous {
                return current;
            }
            assert!(
                Instant::now() < deadline,
                "a successful discovery was never installed: still serving {previous}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The rotation-storm case: one discovery for the whole burst, nobody held longer than the
    /// request budget, and the result installed even though every caller had given up by then.
    ///
    /// That last assertion is the regression guard for the `watch` channel's send semantics. The
    /// only receivers that ever exist are the `rx` locals inside `refresh_jwks_for_request`, so once the
    /// burst has returned the channel has none. A plain `send` fails in exactly that state and —
    /// per its contract — does not hand the value to future receivers either, silently discarding a
    /// successful discovery and leaving the stale keys in place for the whole cooldown.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_of_callers_triggers_one_discovery_whose_result_outlives_them_all() {
        const CALLERS: usize = 250;
        const LATENCY: Duration = Duration::from_millis(1000);
        const BUDGET: Duration = Duration::from_millis(100);

        let (server, mock) = start_mock().await;
        // No cooldown: this test is about the single-flight lock, which has to hold the burst
        // together on its own.
        let instance = instance_for(server, BUDGET, Duration::ZERO).await;
        assert_eq!(mock.discoveries(), 1, "startup runs exactly one discovery");

        let startup_issuer = instance.get_discovered_jwks().get_issuer().to_owned();
        mock.set_latency(LATENCY);

        let elapsed = burst(&instance, CALLERS).await;

        assert_eq!(
            mock.discoveries(),
            2,
            "{CALLERS} callers must produce one discovery, not {CALLERS}"
        );
        assert!(
            elapsed < LATENCY / 2,
            "callers must be bounded by refresh_timeout, not by the discovery: took {elapsed:?}"
        );
        assert_eq!(
            instance.get_discovered_jwks().get_issuer(),
            startup_issuer,
            "the cached keys stand while the refresh is still running"
        );

        let installed = await_installed_issuer(&instance, &startup_issuer, Duration::from_secs(5)).await;
        assert!(
            installed.ends_with("test-2"),
            "the second discovery's result must be installed, got {installed}"
        );
    }

    /// A server that answers with a status instead of a document must be reported as exactly that.
    ///
    /// `Response::json` ignores the status code, so before `require_document` every one of these —
    /// a misspelled realm, a Keycloak that is not up yet, a proxy's HTML error page — came out as
    /// "could not decode payload". Since a failed *startup* discovery aborts the process, that
    /// string was the only thing an operator had to go on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_non_2xx_answer_is_reported_as_a_status_not_as_a_parser_error() {
        let (server, mock) = start_mock().await;
        mock.fail.store(true, Ordering::SeqCst);

        let err = KeycloakAuthInstance::new(config(server, Duration::from_millis(100), Duration::ZERO))
            .await
            .expect_err("startup to fail when the realm does not answer with a document");

        let reported = error_chain(&err);
        assert!(
            reported.contains("404"),
            "the status has to reach the log: {reported}"
        );
        assert!(
            reported.contains("Realm does not exist"),
            "Keycloak's own explanation is the actual diagnosis: {reported}"
        );
        assert!(
            !reported.contains("decode"),
            "a 404 is not a parsing problem: {reported}"
        );
    }

    /// The cooldown is what makes a caller-triggered refresh safe: an unknown `kid` is something a
    /// client controls, so a burst of fabricated ones must not become a request storm.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_cooldown_holds_across_repeated_bursts() {
        const CALLERS: usize = 250;
        const COOLDOWN: Duration = Duration::from_millis(300);

        let (server, mock) = start_mock().await;
        let instance = instance_for(server, Duration::from_millis(100), COOLDOWN).await;

        // `new` stamps the refresh clock, so the instance starts out inside its own cooldown —
        // nothing can have rotated in the moment since.
        burst(&instance, CALLERS).await;
        assert_eq!(
            mock.discoveries(),
            1,
            "a burst inside the cooldown must not reach Keycloak at all"
        );

        tokio::time::sleep(COOLDOWN + Duration::from_millis(50)).await;

        burst(&instance, CALLERS).await;
        assert_eq!(
            mock.discoveries(),
            2,
            "one discovery once the cooldown expired, whoever wins the lock"
        );

        // The winner stamps the clock before running, so the burst it belongs to is already
        // covered by the next cooldown window.
        burst(&instance, CALLERS).await;
        assert_eq!(mock.discoveries(), 2, "the fresh stamp closes the window again");
    }

    /// A failed discovery must leave the previous keys untouched *and* release everyone waiting on
    /// it — that is the `send_modify(|_| {})` in the error arm, which bumps the version without
    /// touching the value. Without it the whole burst would sit out the full `refresh_timeout` for
    /// a result that is never coming.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_discovery_releases_every_caller_and_keeps_the_cached_keys() {
        const CALLERS: usize = 250;
        // Deliberately generous: if the waiters are not woken, the burst takes this long.
        const BUDGET: Duration = Duration::from_secs(5);
        // The single-flight lock only holds a burst together while the discovery it guards is still
        // running. Spawning 250 tasks is not instant, so against a mock that answers immediately the
        // first failure releases the lock before the tail of the burst has even reached it, and a
        // straggler legitimately starts a second discovery — the cooldown that would otherwise
        // absorb it is `ZERO` here. That made the discovery count a race with task startup rather
        // than a statement about the code. A latency longer than the burst takes to fan out pins it.
        const LATENCY: Duration = Duration::from_millis(200);

        let (server, mock) = start_mock().await;
        let instance = instance_for(server, BUDGET, Duration::ZERO).await;
        let startup_issuer = instance.get_discovered_jwks().get_issuer().to_owned();

        mock.fail.store(true, Ordering::SeqCst);
        mock.set_latency(LATENCY);
        let elapsed = burst(&instance, CALLERS).await;

        assert_eq!(mock.discoveries(), 2, "still one discovery for the burst");
        assert!(
            elapsed < BUDGET / 5,
            "a failed discovery must wake its waiters instead of letting them time out: took {elapsed:?}"
        );
        assert_eq!(
            instance.get_discovered_jwks().get_issuer(),
            startup_issuer,
            "a failed refresh must not disturb the keys a successful one left behind"
        );
    }
}