//! Black-box attack suite against a **running** ISM.
//!
//! Where `src/auth/security_tests.rs` verifies the validation rules in-process — signing tokens
//! with a test key and calling the decoder directly — this suite has no access to any of that. It
//! speaks HTTP to a live instance, exactly like an attacker would, and therefore covers what the
//! unit tests structurally cannot: the wiring. Which layer sits in front of which route, whether
//! the configured algorithm allow-list is the one actually in force, whether a rejected request
//! leaks *why* it was rejected, whether the token is read from anywhere but the `Authorization`
//! header, and whether anything in the request can steer the server towards a key set of the
//! caller's choosing.
//!
//! # Running it
//!
//! ```sh
//! docker compose up -d          # Keycloak, PostgreSQL, Redis, MinIO, Redpanda
//! cargo run                     # ISM itself, in another shell
//! cargo test --test auth_attacks -- --nocapture
//! ```
//!
//! `--nocapture` matters: several tests print what they observed (skips, timings, the exact
//! response an attack produced) and cargo swallows that for passing tests otherwise.
//!
//! The target is resolved the same way the server resolves its own listen address — through
//! `ISMConfig::new()`, so it follows `default.config.toml`, the mode file and the `ISM_*`
//! environment overrides. `ISM_ATTACK_BASE_URL` overrides it outright.
//!
//! If ISM or Keycloak is not reachable, every test **skips** rather than fails, so `cargo test`
//! stays green on a machine without the stack. Set `ISM_ATTACK_STRICT=1` to turn every skip into a
//! failure — do that in CI, otherwise a suite that silently tested nothing looks exactly like a
//! suite that passed.
//!
//! # What needs credentials
//!
//! Everything an attacker can do without an account runs unconditionally. Some attacks only mean
//! anything with a *genuine* Keycloak token — an ID token replayed as a bearer credential, a
//! refresh token in the `Authorization` header, a real token with its payload swapped — because
//! forging those without the realm's private key produces a token that dies at the signature check
//! for the wrong reason. Those tests skip unless you point them at a realm account:
//!
//! ```sh
//! ISM_ATTACK_CLIENT_ID=<public client with direct access grants> \
//! ISM_ATTACK_USERNAME=<user> ISM_ATTACK_PASSWORD=<password> \
//! cargo test --test auth_attacks -- --nocapture
//! # optional: ISM_ATTACK_CLIENT_SECRET for a confidential client
//! ```
//!
//! A token minted by a *different* realm is the cross-issuer case; it needs a second account and is
//! configured with the same variables under `ISM_ATTACK_FOREIGN_*` (plus `..._FOREIGN_REALM`).
//!
//! Two tests are `#[ignore]`d because they disturb the environment or take real time:
//!
//! ```sh
//! ISM_ATTACK_ALLOW_DOCKER=1 cargo test --test auth_attacks -- --ignored --nocapture --test-threads=1
//! ```
//!
//! # Reading a failure
//!
//! Every attack asserts the same thing: **401, with the generic body**. A 200 is a breach. A 500 is
//! a parser or panic reached by unauthenticated input. A 503 means the auth layer could not talk to
//! Keycloak — usually the stack, not the code. A 401 whose body differs from every other 401 is an
//! oracle: it tells the sender which check tripped, which is how forged tokens get iterated into
//! working ones.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ism::core::ISMConfig;
use jsonwebtoken::{Algorithm, EncodingKey};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OnceCell;

/// The protected endpoint every attack is fired at. Any route behind the auth layer would do; this
/// one takes no path parameters and no body, so nothing but the token decides the outcome.
const PROTECTED_PATH: &str = "/api/v1/rooms";

/// The rejection every attack must produce, verbatim. See `AuthError::classify`.
const REJECTION_STATUS: u16 = 401;
const REJECTION_CODE: &str = "UNAUTHORIZED";
const REJECTION_MESSAGE: &str = "Authentication required.";

/// `azp` on forged tokens. Irrelevant while `expected_azp` is unset (the default), which is
/// precisely what `rejects_claim_manipulation` documents.
const FORGED_AZP: &str = "ism-attack-suite";

/// A UUID-shaped subject, so `sub` parsing is never what a forged token dies on — the point is to
/// reach the check under test.
const FORGED_SUBJECT: &str = "0193f3a0-1c2d-7e4f-8a9b-0c1d2e3f4a5b";

// ── Target discovery ────────────────────────────────────────────────────────

/// Everything the suite needs to know about the instance under attack.
struct Target {
    /// e.g. `http://127.0.0.1:7800`
    base_url: String,
    /// `host:port` of `base_url`, for the raw-socket requests.
    authority: String,
    /// The issuer Keycloak advertises — not one rebuilt from config, which is what ISM pins against.
    issuer: String,
    token_endpoint: String,
    /// First accepted audience from the config. Forged tokens carry it so `aud` is never the reason
    /// they are rejected.
    audience: String,
    /// Keycloak's active RSA signing key: the `kid` a genuine token names, and the public modulus
    /// and exponent behind it. Public information — which is the entire premise of the RS256→HS256
    /// confusion attack.
    keycloak_kid: String,
    keycloak_n: Vec<u8>,
    keycloak_e: Vec<u8>,
    realm: String,
    http: reqwest::Client,
}

static TARGET: OnceCell<Option<Target>> = OnceCell::const_new();

async fn target() -> Option<&'static Target> {
    TARGET.get_or_init(discover_target).await.as_ref()
}

async fn discover_target() -> Option<Target> {
    let config = match ISMConfig::new() {
        Ok(config) => config,
        Err(err) => {
            println!("Could not load the configuration from the package root: {err}");
            return None;
        }
    };

    let base_url = std::env::var("ISM_ATTACK_BASE_URL").unwrap_or_else(|_| format!("http://{}:{}", config.ism_url, config.ism_port));
    let base_url = base_url.trim_end_matches('/').to_owned();
    let authority = base_url.split_once("://").map_or_else(|| base_url.clone(), |(_, rest)| rest.to_owned());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        // A system proxy that swallows loopback traffic would turn every attack into a false pass.
        .no_proxy()
        // One connection per request. Keep-alive reuse races the server's idle timeout, and a
        // connection reset on that race is indistinguishable here from the server dropping a
        // request — a flaky failure in a suite whose whole output is "was this refused?".
        .pool_max_idle_per_host(0)
        .build()
        .expect("HTTP client builds");

    // Is anything listening, and is it ISM?
    match http.get(format!("{base_url}/health")).send().await {
        Ok(response) if response.status().as_u16() == 200 => {}
        Ok(response) => {
            println!("{base_url}/health answered {} — not ISM?", response.status());
            return None;
        }
        Err(err) => {
            println!("ISM is not reachable at {base_url}: {err}");
            return None;
        }
    }

    // The realm's own view of itself. ISM pins the issuer to what this document advertises, so
    // rebuilding it from `iss_host`/`iss_realm` would produce forged tokens that fail for a reason
    // we did not intend whenever a frontend URL is configured.
    let issuer_host = config.token_issuer.iss_host.trim_end_matches('/').to_owned();
    let realm = config.token_issuer.iss_realm.clone();
    let discovery_url = format!("{issuer_host}/realms/{realm}/.well-known/openid-configuration");
    let discovery: Value = match http.get(&discovery_url).send().await {
        Ok(response) => match response.json().await {
            Ok(json) => json,
            Err(err) => {
                println!("Keycloak discovery document at {discovery_url} was unreadable: {err}");
                return None;
            }
        },
        Err(err) => {
            println!("Keycloak is not reachable at {discovery_url}: {err}");
            return None;
        }
    };

    let issuer = discovery["issuer"].as_str()?.to_owned();
    let jwks_uri = discovery["jwks_uri"].as_str()?.to_owned();
    let token_endpoint = discovery["token_endpoint"].as_str()?.to_owned();

    let jwks: Value = http.get(&jwks_uri).send().await.ok()?.json().await.ok()?;
    let (keycloak_kid, keycloak_n, keycloak_e) = active_signing_key(&jwks)?;

    Some(Target {
        base_url,
        authority,
        issuer,
        token_endpoint,
        audience: config
            .token_issuer
            .expected_audiences
            .first()
            .cloned()
            .unwrap_or_else(|| String::from("account")),
        keycloak_kid,
        keycloak_n,
        keycloak_e,
        realm,
        http,
    })
}

/// Picks the RSA key Keycloak signs access tokens with out of its JWK set.
fn active_signing_key(jwks: &Value) -> Option<(String, Vec<u8>, Vec<u8>)> {
    for key in jwks["keys"].as_array()? {
        if key["kty"].as_str() != Some("RSA") {
            continue;
        }
        // Keycloak publishes its RSA-OAEP *encryption* key in the same document.
        if matches!(key["use"].as_str(), Some(other) if other != "sig") {
            continue;
        }
        if matches!(key["alg"].as_str(), Some(alg) if !alg.starts_with("RS")) {
            continue;
        }
        let kid = key["kid"].as_str()?.to_owned();
        let n = URL_SAFE_NO_PAD.decode(key["n"].as_str()?).ok()?;
        let e = URL_SAFE_NO_PAD.decode(key["e"].as_str()?).ok()?;
        return Some((kid, n, e));
    }
    None
}

// ── Skipping ────────────────────────────────────────────────────────────────

/// Reports that a test could not run. Fails instead when `ISM_ATTACK_STRICT` is set: a suite that
/// skipped everything is indistinguishable from a suite that passed, which is not a state CI should
/// be able to reach quietly.
fn skip(reason: &str) {
    assert!(std::env::var("ISM_ATTACK_STRICT").is_err(), "SKIPPED under ISM_ATTACK_STRICT: {reason}");
    println!("SKIPPED: {reason}");
}

/// Binds the live target or leaves the test, having said why.
macro_rules! target_or_skip {
    () => {
        match target().await {
            Some(target) => target,
            None => {
                skip("no reachable ISM + Keycloak (see the message above)");
                return;
            }
        }
    };
}

/// Binds a genuine Keycloak token set or leaves the test.
macro_rules! tokens_or_skip {
    ($target:expr) => {
        match genuine_tokens($target).await {
            Some(tokens) => tokens,
            None => {
                skip(
                    "no realm credentials: set ISM_ATTACK_CLIENT_ID / ISM_ATTACK_USERNAME / \
                     ISM_ATTACK_PASSWORD to run the attacks that need a genuine token",
                );
                return;
            }
        }
    };
}

// ── Talking to the target ───────────────────────────────────────────────────

/// One response, reduced to what an attacker learns from it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    status: u16,
    code: String,
    message: String,
    body: String,
}

impl Outcome {
    /// The triple a caller can distinguish responses by. Two different attacks producing two
    /// different fingerprints is an oracle.
    fn fingerprint(&self) -> (u16, String, String) {
        (self.status, self.code.clone(), self.message.clone())
    }
}

/// Sends `token` as a bearer credential to the protected route.
async fn call_protected(target: &Target, token: &str) -> Outcome {
    call_with_headers(target, PROTECTED_PATH, &[("Authorization", token_header(token))]).await
}

fn token_header(token: &str) -> String {
    format!("Bearer {token}")
}

async fn call_with_headers(target: &Target, path: &str, headers: &[(&str, String)]) -> Outcome {
    let send = || async {
        let mut request = target.http.get(format!("{}{path}", target.base_url));
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        request.send().await
    };

    // One retry, for the transport rather than the target: a refused or reset connection says
    // nothing about how the token was judged, and letting it fail the assertion would report a
    // breach that never happened.
    let response = match send().await {
        Ok(response) => response,
        Err(first) => {
            tokio::time::sleep(Duration::from_millis(100)).await;
            send()
                .await
                .unwrap_or_else(|second| panic!("request to {path} could not be sent: {first} (retry: {second})"))
        }
    };

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

    Outcome {
        status,
        code: parsed["errorCode"].as_str().unwrap_or_default().to_owned(),
        message: parsed["message"].as_str().unwrap_or_default().to_owned(),
        body,
    }
}

/// The assertion every attack ends in.
///
/// Checks three separate properties, because they fail independently:
/// 1. the request was refused — a 200 here is the breach the whole suite exists to find;
/// 2. it was refused with the *generic* body, so the rejection is not an oracle;
/// 3. nothing the caller put in the token came back out, so error responses cannot be used to
///    reflect content at a third party or into a log-scraping pipeline.
async fn assert_rejected(target: &Target, attack: &str, token: &str) -> Outcome {
    let outcome = call_protected(target, token).await;

    assert_eq!(
        outcome.status, REJECTION_STATUS,
        "{attack}: expected {REJECTION_STATUS}, got {} — body: {}",
        outcome.status, outcome.body
    );
    assert_eq!(
        outcome.code, REJECTION_CODE,
        "{attack}: rejection carried errorCode {:?} instead of {REJECTION_CODE}, which tells the \
         sender which check tripped — body: {}",
        outcome.code, outcome.body
    );
    assert_eq!(
        outcome.message, REJECTION_MESSAGE,
        "{attack}: rejection message differed from the generic one — body: {}",
        outcome.body
    );
    // Length-gated: a short or empty "token" is a substring of any body by accident, and this
    // check is about reflection, not coincidence.
    assert!(
        token.len() < ECHO_MIN_LEN || !outcome.body.contains(token),
        "{attack}: the response echoed the token back — body: {}",
        outcome.body
    );

    outcome
}

/// Shortest attacker-controlled string worth checking for reflection. Below this, a match says more
/// about the alphabet than about the server.
const ECHO_MIN_LEN: usize = 4;

/// Asserts a rejection *and* that no fragment of attacker-chosen text survived into the response.
async fn assert_rejected_without_echo(target: &Target, attack: &str, token: &str, needle: &str) -> Outcome {
    let outcome = assert_rejected(target, attack, token).await;
    assert!(
        needle.len() < ECHO_MIN_LEN || !outcome.body.contains(needle),
        "{attack}: the response echoed attacker-controlled input ({needle:?}) — body: {}",
        outcome.body
    );
    outcome
}

/// Writes a handcrafted request onto a socket, for the cases a well-behaved HTTP client refuses to
/// produce: control characters inside a header, a header far past any sane size, two `Authorization`
/// headers on one request.
///
/// Returns the status line's code, or `None` when the server answered by closing the connection —
/// itself an acceptable way to refuse.
async fn raw_request(target: &Target, raw: &str) -> Option<u16> {
    let addr = &target.authority;
    let mut stream = TcpStream::connect(addr)
        .await
        .unwrap_or_else(|err| panic!("could not connect to {addr}: {err}"));
    stream
        .write_all(raw.as_bytes())
        .await
        .unwrap_or_else(|err| panic!("could not write to {addr}: {err}"));

    let mut buffer = vec![0_u8; 4096];
    let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buffer)).await;
    let read = read.expect("the server answered or closed within 10s");
    let read = read.ok()?;
    if read == 0 {
        return None;
    }

    let head = String::from_utf8_lossy(&buffer[..read]);
    head.split_whitespace().nth(1)?.parse().ok()
}

fn raw_get(target: &Target, authorization_lines: &str) -> String {
    format!(
        "GET {PROTECTED_PATH} HTTP/1.1\r\nHost: {}\r\n{authorization_lines}Connection: close\r\n\r\n",
        target.authority
    )
}

// ── Forging tokens ──────────────────────────────────────────────────────────

/// A throwaway RSA-2048 key, standing in for "the attacker's own key". Not a secret and used
/// nowhere but here: the point of every token it signs is that ISM must not know it.
const ATTACKER_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQD7q21UQWXygUvE
Ws+NDvTevym687EsUSdOQUFJDL1MESpWFeeRl+z6nlcoOd0AkyAqa652FL/Pir0d
KK2lRtHa1PeNiENJMxjRG+ivfueh1TiRAi6Z5QyquyydDQ0e22K2X0CrfXxxke2h
N9a0YjdNWmUnPSy46QGwulHh2Muc+d0Wj4Trbf2RCPQ1gHKBJBONirmHO+T5KJyT
Q1a0M98wlHrh5rxRwVaKyREFma/5G5osNVnXdeCcVAnlNRQSg62p7PGrmqa+PShW
cVeczAGbIurdU8bOdTgPfuLNoN12jePSzn5bf+lpZ5bNHiK1Or8T2ULtoCOwdx9a
wNylSJ9zAgMBAAECggEAdT6WRufWukTRCu9xfNooavMs2js4YZiHErZk10bXk231
xrgasyHPlawZl5RpaKCiHhEfbERbXbFZTBHM39Af6O5JS8bc7eefmp+BZezdtW+T
lD6rfieOoKVlcd8IK0VyddrnUl06EeC1j2Nno46UC/XeZQrjYFuw3WfXyLsKlJyI
ibyTHDCg3HQm3Y5+y1x2uLJ8vmOOU8q72hvfqM8eoyHMQeGOK2iBEDgmpPaiGtIE
Jpgju7/OxRmBenu0lQ5oszByl+4pVaUUpEAN5tFClQTdO30H0AOkiz26ClwrhR/K
wqBttRRUl+tyQ68lgKVuhsNdaQDB7jA0o61GAwNiQQKBgQD/Bayd3ZXtOw0JaVoT
WrdLG4wiAY/4bFfoQIWISHr6Y4NcTEsMbbvgnIj7UzJ4rXDV1ZYUZ/pUOCUMxURK
YNh8G8GT0WcWKsdRUjTFqf8dMFFRNw2fQZv80895T0EQLrmiUuZR99/A7YRBNxe1
a+Fse5YzS8JrGYltiuTZxq7xwQKBgQD8onZFRYvapb1wggI0heeMT/4RfhBmuMjX
qphlNyOQvuSXz2KT8LodgLwhYD0F8F5LNOigf9Uk0sJ/VhrP1WvoQ10Csr2RUuo7
GlmOFpxD+wEBcis2L3nantZRvjbVt6eEAAIQE++y7VvktG3X3xW9ZgRd2L47iQEh
x3Vmnd/2MwKBgQDadLXllYd07HzCbyjmI3OYN0TXbJczqzuyjHLWx5/xFYXVbtVr
FCU4x17gS+iUT560zn39hQR/WIkEY4eYX1WTGwO76ElyR7ruAomKOZF8I4PFGm/k
2IMTFS5JMIb/occLMhBybu+RiOUeKF963asBDu0fi+pDbGC5IZ3gn74FAQKBgGDW
YWFiLB6Og1P58aByZ3QoQWoxGVZWpF3OvYWmohJcqcDrNI0irCSc8QAWJK3/GhXX
3QeQmIH566XlundKBofMMn3TR8jJsJEhI4zMa+++6f7E5X1qq1m6ospIkDpRoHt/
iUriaXH7e8rpwmUJ1Qp5bVkPuLOXa4CoNP81quBzAoGAIZSk1e/aSn6UGcBsEZpY
FYPIvuWckExme91xq9f5mgdL1bhJ7kzTtTVpP1/7du+82qhioD+BZyob+4sgJeAB
XyACnmWLM2aB3lUI5muQ5YSe7TMrtMBlvqNSFGQmk6/ioT4Bs9THGfUhrjqWvVav
c0iY5r5mMKIEUzAfRGdCyJE=
-----END PRIVATE KEY-----";

/// `kid` the attacker publishes their own key under.
const ATTACKER_KID: &str = "attacker-signing-key";

fn attacker_key() -> EncodingKey {
    EncodingKey::from_rsa_pem(ATTACKER_PRIVATE_KEY_PEM.as_bytes()).expect("the attacker's private key parses")
}

/// The attacker's *public* key, as `(n, e)`. Needed to publish it as a JWK the target could pick
/// up — without which the `jwk`/`jku` injections would fail for the trivial reason that the key in
/// the header does not match the signature, and would prove nothing.
fn attacker_public_components() -> (Vec<u8>, Vec<u8>) {
    let key = attacker_key();
    (jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER
        .key_utils
        .rsa_pub_components_from_private_key)(key.as_bytes())
    .expect("the attacker's public components are derivable")
}

fn attacker_jwk() -> Value {
    let (n, e) = attacker_public_components();
    json!({
        "kty": "RSA",
        "use": "sig",
        "alg": "RS256",
        "kid": ATTACKER_KID,
        "n": URL_SAFE_NO_PAD.encode(n),
        "e": URL_SAFE_NO_PAD.encode(e),
    })
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is past 1970").as_secs() as i64
}

/// A claim set that is plausible in every respect — right issuer, right audience, live expiry,
/// UUID subject, `typ: Bearer` — so that whatever a given attack changes is the only thing wrong
/// with it.
fn claims(target: &Target, overrides: Value) -> Value {
    let mut claims = json!({
        "exp": now() + 300,
        "iat": now(),
        "auth_time": now(),
        "jti": "b7c1e5a2-3f4d-4e5a-9b8c-7d6e5f4a3b2c",
        "iss": target.issuer,
        "aud": target.audience,
        "sub": FORGED_SUBJECT,
        "typ": "Bearer",
        "azp": FORGED_AZP,
        "acr": "1",
        "scope": "openid profile email",
        "realm_access": { "roles": ["USER"] },
        "email_verified": true,
        "preferred_username": "attacker",
        "email": "attacker@example.test",
    });

    let target_object = claims.as_object_mut().expect("claims are an object");
    for (key, value) in overrides.as_object().expect("overrides are an object") {
        match value.is_null() {
            // A `null` override removes the claim, which is how the "missing claim" cases are said.
            true => target_object.remove(key),
            false => target_object.insert(key.clone(), value.clone()),
        };
    }
    claims
}

/// A header naming Keycloak's *real* signing key. Attacks that want to reach the signature check
/// must use this: an unknown `kid` is rejected before any crypto runs, so a made-up one would
/// short-circuit the very check under test.
fn header_with_real_kid(target: &Target, alg: &str) -> Value {
    json!({ "alg": alg, "typ": "JWT", "kid": target.keycloak_kid })
}

fn b64_json(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).expect("value serializes"))
}

/// Assembles a token and signs it for real, over exactly the bytes that were assembled. Header and
/// claims stay `Value`s rather than typed structs so any shape can be expressed — a numeric `alg`,
/// a missing one, an `exp` sent as a string.
fn signed(header: &Value, claims: &Value, key: &EncodingKey, alg: Algorithm) -> String {
    let signing_input = format!("{}.{}", b64_json(header), b64_json(claims));
    let signature = jsonwebtoken::crypto::sign(signing_input.as_bytes(), key, alg).expect("the forged token signs");
    format!("{signing_input}.{signature}")
}

/// Signed by the attacker's key: internally consistent, verifiable by anyone holding the attacker's
/// public key, and worthless against a server that only trusts the realm's.
fn forged(target: &Target, claims_overrides: Value) -> String {
    signed(
        &header_with_real_kid(target, "RS256"),
        &claims(target, claims_overrides),
        &attacker_key(),
        Algorithm::RS256,
    )
}

/// Header and payload, no signature at all.
fn unsigned(header: &Value, claims: &Value, signature: &str) -> String {
    format!("{}.{}.{signature}", b64_json(header), b64_json(claims))
}

// ── DER / PEM, for the key-confusion secrets ────────────────────────────────
//
// The RS256→HS256 forgery re-signs a token with HMAC, using the *public* key as the shared secret.
// Whether it works on a vulnerable server depends on which byte representation of that key the
// server would hand to the HMAC — so the attack is run against every representation that is
// plausibly in play, rather than one guess.

fn der_length(len: usize) -> Vec<u8> {
    if len < 0x80 {
        return vec![len as u8];
    }
    let bytes = len.to_be_bytes();
    let significant: Vec<u8> = bytes.iter().copied().skip_while(|b| *b == 0).collect();
    let mut out = vec![0x80 | significant.len() as u8];
    out.extend(significant);
    out
}

fn der_tlv(tag: u8, contents: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(der_length(contents.len()));
    out.extend(contents);
    out
}

/// DER INTEGER from a big-endian magnitude: leading zeros dropped, one re-added when the high bit
/// would otherwise make the value negative.
fn der_integer(magnitude: &[u8]) -> Vec<u8> {
    let trimmed: &[u8] = match magnitude.iter().position(|b| *b != 0) {
        Some(first) => &magnitude[first..],
        None => &[0],
    };
    let mut contents = Vec::with_capacity(trimmed.len() + 1);
    if trimmed[0] & 0x80 != 0 {
        contents.push(0x00);
    }
    contents.extend(trimmed);
    der_tlv(0x02, &contents)
}

/// PKCS#1 `RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }`.
fn rsa_pkcs1_der(n: &[u8], e: &[u8]) -> Vec<u8> {
    let mut contents = der_integer(n);
    contents.extend(der_integer(e));
    der_tlv(0x30, &contents)
}

/// X.509 `SubjectPublicKeyInfo` wrapping the PKCS#1 key — the `BEGIN PUBLIC KEY` form, and the one
/// a JWKS-driven verifier most likely reconstructs internally.
fn rsa_spki_der(n: &[u8], e: &[u8]) -> Vec<u8> {
    // AlgorithmIdentifier { OID 1.2.840.113549.1.1.1 (rsaEncryption), NULL }
    const RSA_ENCRYPTION_OID: [u8; 11] = [0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
    let mut algorithm = RSA_ENCRYPTION_OID.to_vec();
    algorithm.extend([0x05, 0x00]);

    let mut key_bits = vec![0x00]; // unused-bits count
    key_bits.extend(rsa_pkcs1_der(n, e));

    let mut contents = der_tlv(0x30, &algorithm);
    contents.extend(der_tlv(0x03, &key_bits));
    der_tlv(0x30, &contents)
}

fn pem(label: &str, der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let body = encoded
        .as_bytes()
        .chunks(64)
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
}

// ── The JWKS honeypot ───────────────────────────────────────────────────────

/// A local HTTP server that serves a JWK set containing the *attacker's* key and counts every
/// connection it receives.
///
/// It is pointed at by the `jku` and `x5u` headers of the forged tokens below. A single hit means
/// the server let a token nominate where its verification keys come from, which is total
/// compromise — and it would be invisible from the status code alone, because the forged tokens
/// would then verify and return 200. The counter catches even a fetch that happens to fail.
struct Honeypot {
    url: String,
    hits: Arc<AtomicUsize>,
}

impl Honeypot {
    async fn start() -> Self {
        let listener = TcpListener::bind::<SocketAddr>(([127, 0, 0, 1], 0).into())
            .await
            .expect("the honeypot binds a loopback port");
        let addr = listener.local_addr().expect("the honeypot has an address");
        let hits = Arc::new(AtomicUsize::new(0));

        let body = json!({ "keys": [attacker_jwk()] }).to_string();
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let body = body.clone();
                tokio::spawn(async move {
                    let mut scratch = vec![0_u8; 2048];
                    let _ = stream.read(&mut scratch).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });

        Self {
            url: format!("http://{addr}/certs"),
            hits,
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

// ── Genuine tokens ──────────────────────────────────────────────────────────

struct Tokens {
    access: String,
    id: Option<String>,
    refresh: Option<String>,
}

struct Credentials {
    realm: Option<String>,
    client_id: String,
    client_secret: Option<String>,
    username: String,
    password: String,
}

fn credentials_from_env(prefix: &str) -> Option<Credentials> {
    Some(Credentials {
        realm: std::env::var(format!("{prefix}REALM")).ok(),
        client_id: std::env::var(format!("{prefix}CLIENT_ID")).ok()?,
        client_secret: std::env::var(format!("{prefix}CLIENT_SECRET")).ok(),
        username: std::env::var(format!("{prefix}USERNAME")).ok()?,
        password: std::env::var(format!("{prefix}PASSWORD")).ok()?,
    })
}

/// Direct access grant against Keycloak. `scope=openid` is what makes it hand out an ID token as
/// well, which is one of the credentials that must *not* be accepted as a bearer token.
async fn login(target: &Target, credentials: &Credentials) -> Option<Tokens> {
    let endpoint = match &credentials.realm {
        Some(realm) if *realm != target.realm => target
            .token_endpoint
            .replace(&format!("/realms/{}/", target.realm), &format!("/realms/{realm}/")),
        _ => target.token_endpoint.clone(),
    };

    let mut form = vec![
        ("grant_type", "password"),
        ("client_id", credentials.client_id.as_str()),
        ("username", credentials.username.as_str()),
        ("password", credentials.password.as_str()),
        ("scope", "openid"),
    ];
    if let Some(secret) = &credentials.client_secret {
        form.push(("client_secret", secret.as_str()));
    }

    let response = target.http.post(&endpoint).form(&form).send().await.ok()?;
    let status = response.status();
    let payload: Value = response.json().await.ok()?;
    if !status.is_success() {
        println!("Keycloak refused the direct access grant at {endpoint}: {status} {payload}");
        return None;
    }

    Some(Tokens {
        access: payload["access_token"].as_str()?.to_owned(),
        id: payload["id_token"].as_str().map(str::to_owned),
        refresh: payload["refresh_token"].as_str().map(str::to_owned),
    })
}

static GENUINE: OnceCell<Option<Tokens>> = OnceCell::const_new();

async fn genuine_tokens(target: &'static Target) -> Option<&'static Tokens> {
    GENUINE
        .get_or_init(|| async {
            let credentials = credentials_from_env("ISM_ATTACK_")?;
            login(target, &credentials).await
        })
        .await
        .as_ref()
}

// ── Control: is this actually a protected ISM? ──────────────────────────────

#[tokio::test]
async fn the_target_is_a_live_ism_behind_the_auth_layer() {
    // Every rejection assertion below is worthless if the route was never protected, or if the
    // thing answering is not ISM. This is what makes the rest of the suite mean something.
    let target = target_or_skip!();

    let health = call_with_headers(target, "/health", &[]).await;
    assert_eq!(health.status, 200, "/health must be public");

    let unauthenticated = call_with_headers(target, PROTECTED_PATH, &[]).await;
    assert_eq!(
        unauthenticated.status, REJECTION_STATUS,
        "{PROTECTED_PATH} answered {} without a token — it is not behind the auth layer",
        unauthenticated.status
    );
    assert_eq!(unauthenticated.code, REJECTION_CODE);

    println!("Target: {} | issuer: {} | signing kid: {}", target.base_url, target.issuer, target.keycloak_kid);
}

#[tokio::test]
async fn rejects_credentials_that_are_not_bearer_tokens() {
    let target = target_or_skip!();

    for (name, header) in [
        ("basic auth", String::from("Basic YWRtaW46YWRtaW4=")),
        ("bare token, no scheme", forged(target, json!({}))),
        (
            // The extractor matches `Bearer ` exactly. Documented here so a future relaxation to
            // case-insensitive matching is a deliberate, visible change.
            "lowercase scheme",
            format!("bearer {}", forged(target, json!({}))),
        ),
        ("scheme only", String::from("Bearer")),
        ("empty value", String::new()),
        ("negotiate", String::from("Negotiate YII=")),
    ] {
        let outcome = call_with_headers(target, PROTECTED_PATH, &[("Authorization", header)]).await;
        assert_eq!(
            outcome.status, REJECTION_STATUS,
            "{name}: expected {REJECTION_STATUS}, got {} — body: {}",
            outcome.status, outcome.body
        );
    }
}

// ── Algorithm confusion ─────────────────────────────────────────────────────

#[tokio::test]
async fn rejects_unsigned_tokens() {
    // The unsigned-token attack: claim there is no algorithm and append an empty signature. Every
    // spelling, because a case-insensitive comparison somewhere is exactly how this gets through.
    let target = target_or_skip!();

    for alg in ["none", "None", "NONE", "nOnE", "nonE", ""] {
        let header = json!({ "alg": alg, "typ": "JWT", "kid": target.keycloak_kid });
        let payload = claims(target, json!({}));

        for (variant, signature) in [
            ("empty signature", ""),
            ("garbage signature", "aGVsbG8"),
            // A signature copied off a genuine-looking token, in case emptiness is what is checked.
            ("borrowed signature", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ] {
            let token = unsigned(&header, &payload, signature);
            assert_rejected(target, &format!("alg={alg:?} with a {variant}"), &token).await;
        }
    }

    // ...and the same with no `alg` at all, or one that is not even a string.
    for header in [
        json!({ "typ": "JWT", "kid": target.keycloak_kid }),
        json!({ "alg": null, "typ": "JWT", "kid": target.keycloak_kid }),
        json!({ "alg": 0, "typ": "JWT", "kid": target.keycloak_kid }),
        json!({ "alg": ["none", "RS256"], "typ": "JWT", "kid": target.keycloak_kid }),
        json!({ "alg": "RS256 ", "typ": "JWT", "kid": target.keycloak_kid }),
        json!({ "alg": "rs256", "typ": "JWT", "kid": target.keycloak_kid }),
    ] {
        let token = unsigned(&header, &claims(target, json!({})), "");
        assert_rejected(target, &format!("malformed alg: {header}"), &token).await;
    }
}

#[tokio::test]
async fn rejects_rs256_to_hs256_key_confusion() {
    // The classic. Keycloak's signing key is public — it is published at the JWKS endpoint — so if
    // the verifier takes its algorithm from the token header, anyone can re-sign a token of their
    // choosing with HMAC, keyed by that public key, and be believed.
    //
    // The token names the real `kid`, so the server looks up the real RSA key; the only thing
    // deciding the outcome is whether the header's `alg` is allowed to select the algorithm.
    let target = target_or_skip!();

    let (n, e) = (target.keycloak_n.as_slice(), target.keycloak_e.as_slice());
    let spki_der = rsa_spki_der(n, e);
    let pkcs1_der = rsa_pkcs1_der(n, e);

    // Every representation of that public key a vulnerable implementation might use as the secret.
    let secrets: Vec<(&str, Vec<u8>)> = vec![
        ("SPKI PEM", pem("PUBLIC KEY", &spki_der).into_bytes()),
        ("PKCS#1 PEM", pem("RSA PUBLIC KEY", &pkcs1_der).into_bytes()),
        ("SPKI DER", spki_der.clone()),
        ("PKCS#1 DER", pkcs1_der.clone()),
        ("raw modulus", n.to_vec()),
        ("base64url modulus", URL_SAFE_NO_PAD.encode(n).into_bytes()),
        (
            "JWK JSON",
            json!({ "kty": "RSA", "n": URL_SAFE_NO_PAD.encode(n), "e": URL_SAFE_NO_PAD.encode(e) })
                .to_string()
                .into_bytes(),
        ),
    ];

    for (algorithm_name, algorithm) in [("HS256", Algorithm::HS256), ("HS384", Algorithm::HS384), ("HS512", Algorithm::HS512)] {
        for (description, secret) in &secrets {
            let token = signed(
                &header_with_real_kid(target, algorithm_name),
                &claims(target, json!({})),
                &EncodingKey::from_secret(secret),
                algorithm,
            );
            assert_rejected(target, &format!("{algorithm_name} signed with the public key as {description}"), &token).await;
        }
    }

    // The same confusion one family over: an EC or EdDSA header against an RSA key set.
    for alg in ["ES256", "ES384", "PS256", "EdDSA", "RS384", "RS512"] {
        let token = signed(
            &header_with_real_kid(target, alg),
            &claims(target, json!({})),
            &attacker_key(),
            Algorithm::RS256,
        );
        assert_rejected(target, &format!("{alg} header over an RS256 signature"), &token).await;
    }
}

// ── Header injection ────────────────────────────────────────────────────────

#[tokio::test]
async fn rejects_kid_header_injection() {
    // `kid` is read before a single signature is verified, so its content is entirely
    // attacker-chosen. Anything that resolves it — a file path, a cache key, a database lookup, a
    // log field — inherits that. Each payload here targets one such sink; all of them must produce
    // the same flat rejection, and none of them may come back out in the response.
    let target = target_or_skip!();

    let payloads = [
        "../../../../dev/null",
        "../../../../../../etc/passwd",
        "..\\..\\..\\..\\windows\\win.ini",
        "/dev/null",
        "file:///dev/null",
        "' OR '1'='1",
        "'; DROP TABLE app_user;--",
        "\" OR \"\"=\"",
        "1 UNION SELECT null,null--",
        "${jndi:ldap://127.0.0.1:1389/a}",
        "{{7*7}}",
        "%00",
        "\u{0}truncated",
        "real-key\nlevel=ERROR msg=\"forged log line\"",
        "real-key\r\nSet-Cookie: session=stolen",
        "<script>alert(1)</script>",
        "",
        "null",
        "undefined",
    ];

    for payload in payloads {
        let header = json!({ "alg": "RS256", "typ": "JWT", "kid": payload });
        let token = signed(&header, &claims(target, json!({})), &attacker_key(), Algorithm::RS256);
        assert_rejected_without_echo(target, &format!("kid = {payload:?}"), &token, payload).await;
    }

    // An unbounded `kid` is a log-volume amplifier: one request, as much output as the sender likes.
    for length in [1024_usize, 8 * 1024, 64 * 1024] {
        let payload = "A".repeat(length);
        let header = json!({ "alg": "RS256", "typ": "JWT", "kid": payload });
        let token = signed(&header, &claims(target, json!({})), &attacker_key(), Algorithm::RS256);
        assert_rejected(target, &format!("{length} byte kid"), &token).await;
    }
}

#[tokio::test]
async fn never_fetches_verification_keys_named_by_the_token() {
    // `jku`, `x5u` and `jwk` let a token say where its own verification key lives. Honouring any of
    // them is unconditional compromise, and it is also an SSRF primitive: the server would fetch a
    // URL of the caller's choosing from inside its own network.
    //
    // A status code alone cannot prove this — a server might fetch the URL and still reject the
    // token. So the URLs point at a local listener that counts connections, and the assertion is
    // that it was never touched.
    let target = target_or_skip!();
    let honeypot = Honeypot::start().await;

    let attacker_headers = [
        json!({ "alg": "RS256", "typ": "JWT", "kid": ATTACKER_KID, "jku": honeypot.url }),
        json!({ "alg": "RS256", "typ": "JWT", "kid": ATTACKER_KID, "x5u": honeypot.url }),
        json!({ "alg": "RS256", "typ": "JWT", "kid": ATTACKER_KID, "jwk": attacker_jwk() }),
        // The `jwk` smuggled in under the real `kid`, in case the key is looked up by `kid` and the
        // embedded key is then trusted for it.
        json!({ "alg": "RS256", "typ": "JWT", "kid": target.keycloak_kid, "jwk": attacker_jwk() }),
        json!({
            "alg": "RS256",
            "typ": "JWT",
            "kid": ATTACKER_KID,
            "jku": honeypot.url,
            "x5u": honeypot.url,
            "jwk": attacker_jwk(),
        }),
        // Relative and scheme-relative forms, which sometimes slip past an allow-list built on
        // string prefixes.
        json!({ "alg": "RS256", "typ": "JWT", "kid": ATTACKER_KID, "jku": format!("{}/../../{}", target.issuer, honeypot.url) }),
    ];

    for header in attacker_headers {
        let token = signed(&header, &claims(target, json!({})), &attacker_key(), Algorithm::RS256);
        assert_rejected(target, &format!("key-source injection: {header}"), &token).await;
    }

    // Give an in-flight fetch a moment to land before reading the counter.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        honeypot.hits(),
        0,
        "the server fetched a key URL named by the token ({} hits on {}) — a token must never be \
         able to nominate its own verification keys",
        honeypot.hits(),
        honeypot.url
    );
}

// ── Claim manipulation ──────────────────────────────────────────────────────

#[tokio::test]
async fn rejects_claim_manipulation() {
    // Read this one for what it is: without the realm's private key these tokens all carry an
    // invalid signature, so a rejection does not prove the *claim* rule fired — only that nothing
    // about the claim shape (a string `exp`, an absent `aud`, an array where a string belongs)
    // makes the pipeline fall over or take a shortcut before the signature is checked.
    //
    // The claim rules themselves are proven in `src/auth/security_tests.rs`, where the token is
    // signed by a key the verifier trusts and the claim is genuinely the only thing wrong.
    let target = target_or_skip!();

    let cases = [
        ("expired an hour ago", json!({ "exp": now() - 3600 })),
        ("expired just now", json!({ "exp": now() - 30 })),
        ("no exp at all", json!({ "exp": null })),
        ("exp = 0", json!({ "exp": 0 })),
        ("negative exp", json!({ "exp": -1 })),
        ("exp as a string", json!({ "exp": "9999999999" })),
        ("exp beyond i64", json!({ "exp": 9_223_372_036_854_775_807_i64 })),
        ("exp as a float", json!({ "exp": 1.0e18 })),
        ("nbf in the future", json!({ "nbf": now() + 3600 })),
        ("iat in the future", json!({ "iat": now() + 3600 })),
        ("no issuer", json!({ "iss": null })),
        (
            "foreign realm",
            json!({ "iss": format!("{}/realms/master", target.issuer.rsplit_once("/realms/").map(|(host, _)| host.to_owned()).unwrap_or_default()) }),
        ),
        ("attacker issuer", json!({ "iss": "https://attacker.example/realms/meventure" })),
        ("issuer with a trailing slash", json!({ "iss": format!("{}/", target.issuer) })),
        ("issuer as an array", json!({ "iss": [target.issuer.clone()] })),
        ("no audience", json!({ "aud": null })),
        ("foreign audience", json!({ "aud": "some-other-service" })),
        ("empty audience array", json!({ "aud": [] })),
        (
            "audience smuggled into an array",
            json!({ "aud": ["some-other-service", target.audience.clone()] }),
        ),
        ("another realm client", json!({ "azp": "account-console" })),
        ("no azp", json!({ "azp": null })),
        ("no subject", json!({ "sub": null })),
        ("path traversal subject", json!({ "sub": "../../admin" })),
        ("non-uuid subject", json!({ "sub": "admin" })),
        ("sql injection subject", json!({ "sub": "1' OR '1'='1" })),
        ("subject as a number", json!({ "sub": 1 })),
        ("id token replayed", json!({ "typ": "ID" })),
        ("refresh token replayed", json!({ "typ": "Refresh" })),
        ("offline token replayed", json!({ "typ": "Offline" })),
        ("no token type", json!({ "typ": null })),
        ("self-granted realm admin", json!({ "realm_access": { "roles": ["ADMIN", "USER"] } })),
        (
            "admin borrowed from another client",
            json!({ "resource_access": { "some-other-service": { "roles": ["ADMIN"] } } }),
        ),
        ("roles as a string", json!({ "realm_access": { "roles": "ADMIN" } })),
    ];

    for (name, overrides) in cases {
        let token = forged(target, overrides);
        assert_rejected(target, name, &token).await;
    }
}

// ── Structural attacks ──────────────────────────────────────────────────────

#[tokio::test]
async fn rejects_structurally_broken_tokens() {
    let target = target_or_skip!();

    let genuine_shape = forged(target, json!({}));
    let mut parts = genuine_shape.split('.');
    let header = parts.next().expect("header").to_owned();
    let payload = parts.next().expect("payload").to_owned();
    let signature = parts.next().expect("signature").to_owned();

    let claims_json = serde_json::to_vec(&claims(target, json!({}))).expect("claims serialize");

    // A payload nested deeply enough to blow a recursive JSON parser's stack. It never reaches the
    // parser on a correct server — the signature check comes first — but a 500 here would say it did.
    let deep_json = format!("{}1{}", "[".repeat(20_000), "]".repeat(20_000));

    let tokens = [
        ("empty", String::new()),
        ("a single dot", String::from(".")),
        ("two dots", String::from("..")),
        ("four dots", String::from("....")),
        ("not a jwt", String::from("not-a-jwt")),
        ("two segments", format!("{header}.{payload}")),
        ("signature stripped", format!("{header}.{payload}.")),
        ("trailing dot removed", format!("{header}.{payload}")),
        ("four segments", format!("{genuine_shape}.{signature}")),
        ("signature swapped for the header", format!("{header}.{payload}.{header}")),
        ("payload and signature swapped", format!("{header}.{signature}.{payload}")),
        ("standard base64 padding", format!("{header}=.{payload}=.{signature}=")),
        ("non-base64 characters", format!("{header}!.{payload}?.{signature}#")),
        ("whitespace inside", format!("{header} . {payload} . {signature}")),
        ("header only", header.clone()),
        (
            "payload is a json array",
            format!("{header}.{}.{signature}", URL_SAFE_NO_PAD.encode(b"[1,2,3]")),
        ),
        (
            "payload is a json string",
            format!("{header}.{}.{signature}", URL_SAFE_NO_PAD.encode(b"\"admin\"")),
        ),
        ("payload is not json", format!("{header}.{}.{signature}", URL_SAFE_NO_PAD.encode(b"admin"))),
        (
            "payload nested 20k deep",
            format!("{header}.{}.{signature}", URL_SAFE_NO_PAD.encode(deep_json.as_bytes())),
        ),
        ("header is not json", format!("{}.{payload}.{signature}", URL_SAFE_NO_PAD.encode(b"not-json"))),
        ("header is an empty object", format!("{}.{payload}.{signature}", URL_SAFE_NO_PAD.encode(b"{}"))),
        (
            "claims re-encoded with a valid signature over the old ones",
            format!("{header}.{}.{signature}", URL_SAFE_NO_PAD.encode(&claims_json)),
        ),
    ];

    for (name, token) in tokens {
        assert_rejected(target, name, &token).await;
    }
}

#[tokio::test]
async fn oversized_tokens_are_refused_without_stalling() {
    // Unbounded input in the one header that is parsed before authentication. The server may answer
    // 401, 400, 431 or simply close the connection — all fine. What it must not do is accept it,
    // fall over with a 5xx, or hold the connection open while it thinks about it.
    let target = target_or_skip!();

    for size in [8 * 1024_usize, 64 * 1024, 512 * 1024, 1024 * 1024] {
        let padding = "A".repeat(size);
        let token = signed(
            &header_with_real_kid(target, "RS256"),
            &claims(target, json!({ "padding": padding })),
            &attacker_key(),
            Algorithm::RS256,
        );

        let started = Instant::now();
        let status = raw_request(target, &raw_get(target, &format!("Authorization: Bearer {token}\r\n"))).await;
        let elapsed = started.elapsed();

        println!("{size} byte token → {status:?} in {elapsed:?}");
        if let Some(status) = status {
            assert_ne!(status, 200, "a {size} byte token was accepted");
            assert!(
                status < 500,
                "a {size} byte token produced {status} — unauthenticated input must not reach a \
                 server error"
            );
        }
        assert!(elapsed < Duration::from_secs(10), "a {size} byte token held the connection for {elapsed:?}");
    }
}

#[tokio::test]
async fn rejects_malformed_authorization_headers_at_the_wire_level() {
    // Shapes no HTTP client will produce for you, and which therefore never get exercised by
    // anything but a handcrafted socket.
    let target = target_or_skip!();
    let token = forged(target, json!({}));

    let cases = [
        (
            "two Authorization headers, forged first",
            format!("Authorization: Bearer {token}\r\nAuthorization: Bearer {token}\r\n"),
        ),
        (
            "Authorization with a folded continuation line",
            format!("Authorization: Bearer\r\n\t{token}\r\n"),
        ),
        ("Bearer with a tab separator", format!("Authorization:\tBearer\t{token}\r\n")),
        ("two spaces after the scheme", format!("Authorization: Bearer  {token}\r\n")),
        ("trailing whitespace", format!("Authorization: Bearer {token} \r\n")),
        ("duplicated scheme", format!("Authorization: Bearer Bearer {token}\r\n")),
        ("header name in mixed case", format!("AUTHORIZATION: Bearer {token}\r\n")),
        ("non-ascii in the value", String::from("Authorization: Bearer ünïcödé.tökén.hërë\r\n")),
    ];

    for (name, header_lines) in cases {
        let status = raw_request(target, &raw_get(target, &header_lines)).await;
        println!("{name} → {status:?}");
        match status {
            // Closing the connection is a legitimate refusal.
            None => {}
            Some(status) => {
                assert_ne!(status, 200, "{name} was accepted");
                assert!(status < 500, "{name} produced {status}");
            }
        }
    }
}

// ── JWKS handling ───────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_signing_keys_do_not_become_a_request_storm() {
    // An unknown `kid` is the one failure a key rotation produces, so it is the one failure worth
    // re-running OIDC discovery for — which makes it a caller-controlled trigger. Fifty fabricated
    // `kid`s must not turn into fifty discoveries, must not exhaust anything, and must not start
    // producing 503s: a service that answers "auth unavailable" under a trivial flood is a
    // denial-of-service away from being down for everyone.
    let target = target_or_skip!();

    let started = Instant::now();
    let mut slowest = Duration::ZERO;

    for index in 0..50 {
        let kid = format!("rotated-{}-{index}", SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos());
        let header = json!({ "alg": "RS256", "typ": "JWT", "kid": kid });
        let token = signed(&header, &claims(target, json!({})), &attacker_key(), Algorithm::RS256);

        let request_started = Instant::now();
        let outcome = call_protected(target, &token).await;
        slowest = slowest.max(request_started.elapsed());

        assert_eq!(
            outcome.status, REJECTION_STATUS,
            "unknown kid #{index} produced {} — body: {}",
            outcome.status, outcome.body
        );
    }

    println!("50 unknown-kid requests in {:?}, slowest single request {:?}", started.elapsed(), slowest);

    // The service is still healthy afterwards.
    let health = call_with_headers(target, "/health", &[]).await;
    assert_eq!(health.status, 200, "the flood left /health unhealthy");
}

#[tokio::test]
async fn rejections_are_indistinguishable_from_one_another() {
    // The reason a token was refused is information: "wrong audience" narrows the search space,
    // "bad signature" tells you the claims were fine. Every class of failure must therefore look
    // identical from outside — same status, same code, same message.
    //
    // Expiry is deliberately not in this set: `TOKEN_EXPIRED` is distinguishable on purpose, so a
    // client knows to refresh rather than to re-authenticate.
    let target = target_or_skip!();

    let mut fingerprints: BTreeSet<(u16, String, String)> = BTreeSet::new();
    let mut observed: Vec<(&str, Outcome)> = Vec::new();

    let attacks: Vec<(&str, String)> = vec![
        ("garbage", String::from("not-a-jwt")),
        ("alg none", unsigned(&header_with_real_kid(target, "none"), &claims(target, json!({})), "")),
        ("bad signature", forged(target, json!({}))),
        (
            "unknown kid",
            signed(
                &json!({ "alg": "RS256", "typ": "JWT", "kid": "no-such-key" }),
                &claims(target, json!({})),
                &attacker_key(),
                Algorithm::RS256,
            ),
        ),
        ("foreign issuer", forged(target, json!({ "iss": "https://attacker.example/realms/x" }))),
        ("foreign audience", forged(target, json!({ "aud": "some-other-service" }))),
        ("id token", forged(target, json!({ "typ": "ID" }))),
        ("no subject", forged(target, json!({ "sub": null }))),
    ];

    for (name, token) in &attacks {
        let outcome = call_protected(target, token).await;
        fingerprints.insert(outcome.fingerprint());
        observed.push((name, outcome));
    }

    // ...and the no-credentials case, which must look the same as a bad credential.
    let missing = call_with_headers(target, PROTECTED_PATH, &[]).await;
    fingerprints.insert(missing.fingerprint());
    observed.push(("no authorization header", missing));

    assert_eq!(
        fingerprints.len(),
        1,
        "rejections are distinguishable, which turns the endpoint into an oracle for crafting \
         tokens: {observed:#?}"
    );
}

// ── Attacks that need a genuine token ───────────────────────────────────────

#[tokio::test]
async fn a_genuine_access_token_is_accepted() {
    // The control for every test below it: without this, "rejected" proves nothing, because a
    // server that rejects everything passes the entire suite.
    let target = target_or_skip!();
    let tokens = tokens_or_skip!(target);

    let outcome = call_protected(target, &tokens.access).await;
    assert!(
        outcome.status != 401 && outcome.status != 403,
        "a genuine Keycloak access token was refused with {} — body: {}",
        outcome.status,
        outcome.body
    );
    println!("genuine access token → {} (auth passed)", outcome.status);
}

#[tokio::test]
async fn rejects_other_genuine_keycloak_tokens_as_bearer_credentials() {
    // ID and refresh tokens come from the same realm and, in the ID token's case, from the same
    // signing key — every signature and claim check passes. Only `typ` separates a bearer
    // credential from something that was never meant to be one.
    let target = target_or_skip!();
    let tokens = tokens_or_skip!(target);

    match &tokens.id {
        Some(id_token) => {
            assert_rejected(target, "id token as a bearer credential", id_token).await;
        }
        None => println!("no id_token in the grant response (add `openid` to the client's scopes)"),
    }

    match &tokens.refresh {
        Some(refresh_token) => {
            assert_rejected(target, "refresh token as a bearer credential", refresh_token).await;
        }
        None => println!("no refresh_token in the grant response"),
    }
}

#[tokio::test]
async fn rejects_a_tampered_genuine_token() {
    // Take a token the server *does* accept and change one thing about it. This is the only way to
    // prove the signature is verified at all rather than merely parsed.
    let target = target_or_skip!();
    let tokens = tokens_or_skip!(target);

    let mut parts = tokens.access.split('.');
    let header = parts.next().expect("header").to_owned();
    let payload = parts.next().expect("payload").to_owned();
    let signature = parts.next().expect("signature").to_owned();

    let decoded = URL_SAFE_NO_PAD.decode(&payload).expect("the genuine payload is base64url");
    let mut genuine_claims: Value = serde_json::from_slice(&decoded).expect("the genuine payload is json");

    // Become someone else, keeping the genuine signature.
    let claims_object = genuine_claims.as_object_mut().expect("claims are an object");
    claims_object.insert(String::from("sub"), json!("ffffffff-ffff-4fff-8fff-ffffffffffff"));
    let elevated = URL_SAFE_NO_PAD.encode(genuine_claims.to_string());

    assert_rejected(target, "genuine token, subject swapped", &format!("{header}.{elevated}.{signature}")).await;

    // Grant yourself a realm role.
    let mut escalated_claims: Value = serde_json::from_slice(&decoded).expect("the genuine payload is json");
    escalated_claims
        .as_object_mut()
        .expect("claims are an object")
        .insert(String::from("realm_access"), json!({ "roles": ["ADMIN"] }));
    assert_rejected(
        target,
        "genuine token, realm roles rewritten",
        &format!("{header}.{}.{signature}", URL_SAFE_NO_PAD.encode(escalated_claims.to_string())),
    )
    .await;

    // Same claims, no signature.
    assert_rejected(target, "genuine token with the signature stripped", &format!("{header}.{payload}.")).await;

    // Same claims, re-declared as unsigned.
    let mut genuine_header: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&header).expect("the genuine header is base64url")).expect("the genuine header is json");
    genuine_header
        .as_object_mut()
        .expect("the header is an object")
        .insert(String::from("alg"), json!("none"));
    assert_rejected(
        target,
        "genuine claims re-declared as alg=none",
        &format!("{}.{payload}.", b64_json(&genuine_header)),
    )
    .await;

    // Flipped bits inside the signature itself.
    let mut broken = signature.clone().into_bytes();
    if let Some(last) = broken.last_mut() {
        *last = if *last == b'A' { b'B' } else { b'A' };
    }
    assert_rejected(
        target,
        "genuine token with one signature byte changed",
        &format!("{header}.{payload}.{}", String::from_utf8(broken).expect("still ascii")),
    )
    .await;
}

#[tokio::test]
async fn does_not_read_the_token_from_the_query_string() {
    // ISM wires up the header extractor only. A token in a URL leaks into access logs, proxy logs,
    // browser history and `Referer` headers, so if the WebSocket route ever needs the query
    // extractor, this test is what will notice it being switched on globally.
    let target = target_or_skip!();
    let tokens = tokens_or_skip!(target);

    for parameter in ["token", "access_token", "jwt", "bearer", "auth"] {
        let path = format!("{PROTECTED_PATH}?{parameter}={}", tokens.access);
        let outcome = call_with_headers(target, &path, &[]).await;
        assert_eq!(
            outcome.status, REJECTION_STATUS,
            "a genuine token in ?{parameter}= authenticated the request ({}) — body: {}",
            outcome.status, outcome.body
        );
    }
}

#[tokio::test]
async fn rejects_a_genuine_token_from_another_realm() {
    // A correctly signed, unexpired, entirely legitimate token — from the wrong issuer. Nothing
    // about it is malformed; only the `iss` pin and the key set stand between it and access.
    let target = target_or_skip!();

    let Some(credentials) = credentials_from_env("ISM_ATTACK_FOREIGN_") else {
        skip(
            "no second realm configured: set ISM_ATTACK_FOREIGN_REALM / _CLIENT_ID / _USERNAME / \
             _PASSWORD to run the cross-issuer attack",
        );
        return;
    };

    let Some(foreign) = login(target, &credentials).await else {
        skip("the configured second realm did not issue a token");
        return;
    };

    assert_rejected(target, "access token from another realm", &foreign.access).await;
    if let Some(id_token) = &foreign.id {
        assert_rejected(target, "id token from another realm", id_token).await;
    }
}

// ── Opt-in: environment-disturbing and slow checks ──────────────────────────

#[tokio::test]
#[ignore = "stops and restarts the Keycloak container; run with --test-threads=1 and ISM_ATTACK_ALLOW_DOCKER=1"]
async fn a_keycloak_outage_does_not_open_the_door() {
    // The question is which way the service fails when its identity provider disappears. Serving
    // cached keys is correct — a rotation that never happened cannot invalidate them, and dropping
    // every session because Keycloak restarted would be its own outage. Accepting anything it
    // cannot verify is not.
    let target = target_or_skip!();

    if std::env::var("ISM_ATTACK_ALLOW_DOCKER").is_err() {
        skip("set ISM_ATTACK_ALLOW_DOCKER=1 to let this test stop and start the Keycloak container");
        return;
    }

    let genuine = genuine_tokens(target).await.map(|tokens| tokens.access.clone());

    compose(&["stop", "keycloak"]);

    // Collected rather than asserted, so Keycloak is restarted even when something fails.
    let mut failures: Vec<String> = Vec::new();

    let forged_token = forged(target, json!({}));
    let outcome = call_protected(target, &forged_token).await;
    if outcome.status != REJECTION_STATUS {
        failures.push(format!(
            "with Keycloak down, a forged token produced {} — body: {}",
            outcome.status, outcome.body
        ));
    }

    let unknown_kid = signed(
        &json!({ "alg": "RS256", "typ": "JWT", "kid": "rotated-during-the-outage" }),
        &claims(target, json!({})),
        &attacker_key(),
        Algorithm::RS256,
    );
    let outcome = call_protected(target, &unknown_kid).await;
    if outcome.status != REJECTION_STATUS {
        failures.push(format!(
            "with Keycloak down, an unknown kid produced {} instead of {REJECTION_STATUS} — body: {}",
            outcome.status, outcome.body
        ));
    }

    let health = call_with_headers(target, "/health", &[]).await;
    if health.status != 200 {
        failures.push(format!("with Keycloak down, /health answered {}", health.status));
    }

    if let Some(access) = &genuine {
        let outcome = call_protected(target, access).await;
        println!(
            "with Keycloak down, a previously issued genuine token → {} (cached keys still verify \
             it, which is the intended availability behaviour)",
            outcome.status
        );
    }

    compose(&["start", "keycloak"]);

    // Wait for the realm to answer again, so the next test does not run against a half-started
    // Keycloak.
    let discovery_url = format!("{}/.well-known/openid-configuration", target.issuer);
    for _ in 0..60 {
        if target
            .http
            .get(&discovery_url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn compose(arguments: &[&str]) {
    let status = std::process::Command::new("docker")
        .arg("compose")
        .args(arguments)
        .status()
        .expect("docker compose runs");
    assert!(status.success(), "docker compose {arguments:?} failed");
}

#[tokio::test]
#[ignore = "reports timings rather than asserting; run explicitly"]
async fn reports_the_timing_of_each_rejection_path() {
    // Informational on purpose. Over a network, per-request noise dwarfs the difference this would
    // have to detect, so a threshold here would either never fire or flake constantly. What it is
    // good for is spotting an order-of-magnitude gap — a rejection path that hits the network, or a
    // claim compared with a short-circuiting string comparison — which is visible by eye.
    let target = target_or_skip!();

    let samples = 40;
    let probes: Vec<(&str, String)> = vec![
        ("no token", String::new()),
        ("garbage", String::from("not-a-jwt")),
        ("alg none", unsigned(&header_with_real_kid(target, "none"), &claims(target, json!({})), "")),
        ("bad signature, real kid", forged(target, json!({}))),
        (
            "unknown kid",
            signed(
                &json!({ "alg": "RS256", "typ": "JWT", "kid": "no-such-key" }),
                &claims(target, json!({})),
                &attacker_key(),
                Algorithm::RS256,
            ),
        ),
        ("expired", forged(target, json!({ "exp": now() - 3600 }))),
        ("foreign audience", forged(target, json!({ "aud": "elsewhere" }))),
    ];

    for (name, token) in &probes {
        let mut timings = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = Instant::now();
            if token.is_empty() {
                call_with_headers(target, PROTECTED_PATH, &[]).await;
            } else {
                call_protected(target, token).await;
            }
            timings.push(started.elapsed());
        }
        timings.sort_unstable();
        println!("{name:<26} median {:>9?}  p95 {:>9?}", timings[samples / 2], timings[samples * 95 / 100]);
    }
}
