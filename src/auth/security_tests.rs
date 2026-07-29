//! Adversarial tests for the JWT validation path.
//!
//! Each test encodes one attack that the validation must reject. They run entirely in-process:
//! tokens are signed with a test key below and verified through the same code the middleware
//! uses, so no Keycloak and no network are involved.
//!
//! The entry points under test are `RawToken::decode_header` /
//! `RawToken::decode_and_validate` (signature, algorithm, issuer, audience, expiry, required
//! claims) and `parse_claims` (token type, authorized party, subject shape).
//!
//! When adding a validation rule, add the attack it defeats here — a rule with no failing test
//! before it existed is a rule nobody can prove still works.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header};
use serde_json::{Value, json};

use url::Url;

use crate::auth::app_role::AppRole;
use crate::auth::decode::{RawToken, decode_and_validate, parse_claims};
use crate::auth::error::AuthError;
use crate::auth::instance::{JwkDecodingKey, KeycloakAuthInstance, KeycloakConfig, keys_for_kid};
use crate::auth::policy::ValidationPolicy;
use crate::auth::token::{ProfileAndEmail, StandardClaims};

/// Test-only RSA-2048 keypair. Not a secret and never used outside this file.
const TEST_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC6Lsy40iI95RfA
t/V3F3/qaNEfI2qv15zRkCDjwCZyHTbR8YALCO55kcGC1ljWG6crA4f6kfZoarEB
6tfLpQQvQvtIozurIt5oNaZOnMK1jFSbBxLvCzWW6MVpZmOGDDN+bRlhl73kj8bq
sEu/puRBhEeRrIPT6Y0TxZwrH9iX6gULWKBYWhKp/o7+9iq2q28s3aa8h1MAI/Xi
V0+Yvn+gi25IzuRooNh1M6+ONSDjqqtzT/OWikK4ckY+BkzPGA17jE5fMbDO4AGj
yaYU+1K4Pk0qwwYnBwayBkfKnfMVxQEc5QrgsCZLDerzSzpTfXnygMKS8XPkDSwb
6a1Y114PAgMBAAECggEAC6KEEaK0GBkacGUulkgmKsBtHRyJ/L4lIyV2ILVv0Z7I
v7rvTQE8YeV9ac86Uvr8aeA5HawEcYcFU8DYxnWj+s4dRO9KecneizWbFHuQYWcJ
HH0HLmANc8ZNG+aVnplhmGt59BLW/5MKk7z7ptjnl76L+GsG+/Wy5sLpHPrK/sc6
PyLapU5p+QD9KaI2HCOFm/HFMGy48P0A+Gh4tkj8DgNNa2yWDo7bEGtw0SlFi//a
aS4S5thwC4bJKJRQTKxwiA9jiPL9ddpBxyt2E1XR8drzLUjpySqob/mF+al2A3Te
VzxABfNAnING9rq8IafAW+Bc5q9odI2D6Rr0molH6QKBgQDngBn76ypZGrPERcLh
hWeu7uglC4erbPgRuxdr9UHRTJJ9m9lvMD9JiKjm2Y2WwfUsUNQ8BqWtcwwMWyi+
DnUcceDyQLj0lIgjN45h+JaNqUMrgQFtkXy20YxMRQZva5LKpQSERjwpRvqp1YLd
TrhdYNvTEIabfBxWoQx546aZSQKBgQDN4u/Fc6b40pWCZPqrhrxQCv3X0JIOSwKD
haPE1msgV5NWlISZ/zkrtQj43Iddy/xqyVEs7NoOscRR3cXX8yCxr7HSzMXIFTs7
cE+Q0gwdzvLQ43n1NyhDJfTIKQWkTIksbebCqSAJaDgqpPloxuUNVx+O5qEH2gze
8TsLDA9UlwKBgQCsK7ynfEW5kT9jWNLQcSwkkS/75TBYkSmJ3lBUDUqPA9jrLE6w
//wBj262idRg7A2QkOjXX8Y2UpsCUYXim9QDfLpk0Tf9Rr5dGsN9H6mw39LB9yb9
uzc6rGwgiTF5ClNY/RN34Nh7hnuEdfPm7dX2NMQonGDQIKTe1NX3jRTpaQKBgQCc
qjvDZv6+RheockhgbxUqX0LLjxUktSVDiVSV+obnxFwEPN0uBYyuWoJqQ/zpfcgk
Re50HgLLva9ikDv02Defnc7VViaF2soIr6yLyZmYsRoJo57w3jjP57j8+mIlpGuZ
GEPJCkKrhdd/c6upc/dlkE8eQRZ10BGNL8i63kFoHwKBgG2xY1iOJuW/NbtPsN9Z
zdiNso3tYHLzSB3Oxz5B6GguiucHl/kQt8L6Ho4t/LhjIx1YViUPt6ONPi8v42//
7b1HXvk28rxnd5crUPxajuDUaIcECIz0pixnOIigkc+DLrH4tTFc2PIHxmwAUaQ6
3wr2jjWHZYQByCDxwzzU5oA5
-----END PRIVATE KEY-----";

const TEST_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAui7MuNIiPeUXwLf1dxd/
6mjRHyNqr9ec0ZAg48Amch020fGACwjueZHBgtZY1hunKwOH+pH2aGqxAerXy6UE
L0L7SKM7qyLeaDWmTpzCtYxUmwcS7ws1lujFaWZjhgwzfm0ZYZe95I/G6rBLv6bk
QYRHkayD0+mNE8WcKx/Yl+oFC1igWFoSqf6O/vYqtqtvLN2mvIdTACP14ldPmL5/
oItuSM7kaKDYdTOvjjUg46qrc0/zlopCuHJGPgZMzxgNe4xOXzGwzuABo8mmFPtS
uD5NKsMGJwcGsgZHyp3zFcUBHOUK4LAmSw3q80s6U3158oDCkvFz5A0sG+mtWNde
DwIDAQAB
-----END PUBLIC KEY-----";

const KID: &str = "test-signing-key";
const ISSUER: &str = "https://keycloak.example/realms/meventure";
const OTHER_ISSUER: &str = "https://attacker.example/realms/meventure";
const AUDIENCE: &str = "account";
const CLIENT_ID: &str = "ism-app";
const SUBJECT: &str = "0193f3a0-1c2d-7e4f-8a9b-0c1d2e3f4a5b";

// ── Fixtures ────────────────────────────────────────────────────────────────

fn policy() -> ValidationPolicy {
    ValidationPolicy::new(
        ISSUER.to_owned(),
        vec![AUDIENCE.to_owned()],
        vec![],
        &[String::from("RS256")],
    )
    .expect("valid policy")
}

/// Policy that additionally pins the authorized party, i.e. the tightened production setup.
fn policy_pinning_azp() -> ValidationPolicy {
    ValidationPolicy::new(
        ISSUER.to_owned(),
        vec![AUDIENCE.to_owned()],
        vec![CLIENT_ID.to_owned()],
        &[String::from("RS256")],
    )
    .expect("valid policy")
}

fn signing_key() -> EncodingKey {
    EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY_PEM.as_bytes()).expect("private key parses")
}

fn verification_key() -> DecodingKey {
    DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY_PEM.as_bytes()).expect("public key parses")
}

fn now() -> u64 {
    jsonwebtoken::get_current_timestamp()
}

/// A claim set that passes every check, as the baseline each attack deviates from.
fn base_claims() -> Value {
    json!({
        "exp": now() + 300,
        "iat": now(),
        "nbf": now() - 10,
        "jti": "b7c1e5a2-3f4d-4e5a-9b8c-7d6e5f4a3b2c",
        "iss": ISSUER,
        "aud": AUDIENCE,
        "sub": SUBJECT,
        "typ": "Bearer",
        "azp": CLIENT_ID,
        "preferred_username": "tim",
        "email": "tim@example.test",
        "email_verified": true,
    })
}

/// `base_claims` with `overrides` applied. A `null` value removes the claim entirely, which is
/// how the "missing required claim" cases are expressed.
fn claims_with(overrides: Value) -> Value {
    let mut claims = base_claims();
    let target = claims.as_object_mut().expect("claims are an object");
    for (key, value) in overrides.as_object().expect("overrides are an object") {
        match value.is_null() {
            true => target.remove(key),
            false => target.insert(key.clone(), value.clone()),
        };
    }
    claims
}

fn rs256_header() -> Header {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KID.to_owned());
    header
}

fn sign(header: &Header, claims: &Value) -> String {
    jsonwebtoken::encode(header, claims, &signing_key()).expect("token encodes")
}

/// Runs a token through exactly what the middleware runs: header decode, then verification.
fn verify_with(
    token: &str,
    policy: &ValidationPolicy,
) -> Result<StandardClaims<ProfileAndEmail>, AuthError> {
    let raw_token = RawToken { token };
    raw_token.decode_header()?;
    let key = verification_key();
    raw_token.decode_and_validate(policy, &[&key])
}

fn verify(token: &str) -> Result<StandardClaims<ProfileAndEmail>, AuthError> {
    verify_with(token, &policy())
}

/// Full pipeline including the claim-level checks that run after signature verification.
async fn authenticate(token: &str, policy: &ValidationPolicy) -> Result<(), AuthError> {
    authenticate_expecting_roles(token, policy, &[]).await
}

/// `authenticate`, additionally requiring `required_roles` — the layer-wide role gate.
async fn authenticate_expecting_roles(
    token: &str,
    policy: &ValidationPolicy,
    required_roles: &[AppRole],
) -> Result<(), AuthError> {
    let claims = verify_with(token, policy)?;
    parse_claims::<AppRole, ProfileAndEmail>(claims, required_roles, policy).map(|_| ())
}

// ── Control: the baseline must actually pass ────────────────────────────────

#[tokio::test]
async fn accepts_a_well_formed_token() {
    // Without this, every rejection test below could pass for the wrong reason.
    let token = sign(&rs256_header(), &base_claims());
    authenticate(&token, &policy())
        .await
        .expect("a well-formed token is accepted");
}

#[tokio::test]
async fn accepts_matching_authorized_party_when_pinned() {
    let token = sign(&rs256_header(), &base_claims());
    authenticate(&token, &policy_pinning_azp())
        .await
        .expect("the configured client is accepted");
}

// ── Algorithm attacks ───────────────────────────────────────────────────────

#[test]
fn rejects_alg_none() {
    // The unsigned-token attack: claim `alg: none` and append an empty signature.
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(base_claims().to_string());
    let token = format!("{header}.{payload}.");

    let err = verify(&token).expect_err("an unsigned token must be rejected");
    assert!(
        matches!(err, AuthError::DecodeHeader { .. }),
        "expected the header decode to fail, got {err:?}"
    );
}

#[test]
fn rejects_rs256_to_hs256_key_confusion() {
    // The classic: re-sign with HMAC using the *public* key as the shared secret, betting that
    // the verifier picks its algorithm from the token header.
    let header = Header::new(Algorithm::HS256);
    let forged = jsonwebtoken::encode(
        &header,
        &base_claims(),
        &EncodingKey::from_secret(TEST_PUBLIC_KEY_PEM.as_bytes()),
    )
    .expect("forged token encodes");

    let err = verify(&forged).expect_err("an HMAC-signed token must be rejected");
    assert!(
        matches!(err, AuthError::Decode { .. }),
        "expected verification to fail, got {err:?}"
    );
}

#[test]
fn config_refuses_symmetric_and_mixed_algorithm_families() {
    // The forgery above only stays impossible while the allow-list holds no symmetric algorithm,
    // so the policy refuses to be built that way at all.
    assert!(
        ValidationPolicy::new(
            ISSUER.to_owned(),
            vec![AUDIENCE.to_owned()],
            vec![],
            &[String::from("HS256")]
        )
        .is_err()
    );
    assert!(
        ValidationPolicy::new(
            ISSUER.to_owned(),
            vec![AUDIENCE.to_owned()],
            vec![],
            &[String::from("RS256"), String::from("ES256")]
        )
        .is_err()
    );
    assert!(
        ValidationPolicy::new(ISSUER.to_owned(), vec![AUDIENCE.to_owned()], vec![], &[]).is_err()
    );
}

// ── Signature attacks ───────────────────────────────────────────────────────

#[test]
fn rejects_tampered_payload() {
    // Keep a genuine signature, swap the payload underneath it.
    let token = sign(&rs256_header(), &base_claims());
    let mut parts = token.split('.');
    let header = parts.next().expect("header");
    let _original = parts.next().expect("payload");
    let signature = parts.next().expect("signature");

    let escalated = claims_with(json!({ "sub": "ffffffff-ffff-4fff-8fff-ffffffffffff" }));
    let forged = format!(
        "{header}.{}.{signature}",
        URL_SAFE_NO_PAD.encode(escalated.to_string())
    );

    let err = verify(&forged).expect_err("a tampered payload must be rejected");
    assert!(
        matches!(err, AuthError::Decode { .. }),
        "expected verification to fail, got {err:?}"
    );
}

#[test]
fn rejects_token_signed_by_an_unknown_key() {
    // A token that is internally consistent but signed by a key we do not trust.
    let foreign_key = EncodingKey::from_rsa_pem(FOREIGN_PRIVATE_KEY_PEM.as_bytes())
        .expect("foreign private key parses");
    let forged = jsonwebtoken::encode(&rs256_header(), &base_claims(), &foreign_key)
        .expect("forged token encodes");

    let err = verify(&forged).expect_err("a foreign signature must be rejected");
    assert!(
        matches!(err, AuthError::Decode { .. }),
        "expected verification to fail, got {err:?}"
    );
}

// ── Rediscovery trigger: rotation vs. forgery ───────────────────────────────
//
// `decode_and_validate` may re-run OIDC discovery when a token fails to verify, which makes the
// trigger reachable by anyone who can send a request. The signature check cannot decide who
// deserves it: `InvalidSignature` means only "the verify operation returned false", so a forged
// token and a token signed by a rotated key are indistinguishable there. The decision is therefore
// made from the header's `kid`, before any crypto — and these tests pin that split.

/// The cached key set as it looks after a successful discovery: one key, published under `KID`.
fn cached_keys() -> Vec<JwkDecodingKey> {
    vec![JwkDecodingKey {
        kid: Some(KID.to_owned()),
        key: verification_key(),
    }]
}

/// An instance that never contacted Keycloak, for the header checks that must reject a token
/// before the key set is consulted at all.
fn test_instance() -> KeycloakAuthInstance {
    KeycloakAuthInstance::without_discovery(
        KeycloakConfig::builder()
            .server(Url::parse("https://localhost:8443/").expect("valid url"))
            .realm(String::from("meventure"))
            .expected_audiences(vec![AUDIENCE.to_owned()])
            .expected_azp(vec![])
            .allowed_algorithms(vec![String::from("RS256")])
            .build(),
        // Handed over rather than discovered: `without_discovery` never contacts Keycloak, so
        // there is no advertised issuer to derive one from.
        policy(),
    )
}

#[tokio::test]
async fn rejects_token_without_kid_before_touching_the_key_set() {
    // Keycloak stamps a `kid` on every access token. Without one there is nothing to match a key
    // against, so this is refused outright rather than tried against every key we hold — which is
    // what the old fall-back did, turning one such request into N public-key operations.
    let mut header = Header::new(Algorithm::RS256);
    header.kid = None;
    let token = sign(&header, &base_claims());

    let err = decode_and_validate::<ProfileAndEmail>(&test_instance(), RawToken { token: token.as_str() })
        .await
        .expect_err("a token without a kid must be rejected");
    assert!(
        matches!(&err, AuthError::InvalidToken { reason } if reason.contains("kid")),
        "expected rejection for the missing kid, got {err:?}"
    );
}

#[test]
fn a_forgery_reusing_a_known_kid_does_not_look_like_rotation() {
    // The realistic forgery: copy a genuine token's header — `kid` and all — and sign a payload of
    // your own. The verify step fails exactly as a rotated key would, so keying rediscovery off
    // that error let every forged token reach for Keycloak. The `kid` tells them apart: we hold
    // the key it names, so the failure is a property of the token and rediscovery cannot help.
    let foreign_key = EncodingKey::from_rsa_pem(FOREIGN_PRIVATE_KEY_PEM.as_bytes())
        .expect("foreign private key parses");
    let forged = jsonwebtoken::encode(&rs256_header(), &base_claims(), &foreign_key)
        .expect("forged token encodes");

    verify(&forged).expect_err("a foreign signature must be rejected");
    assert!(
        keys_for_kid(&cached_keys(), KID).is_some(),
        "the forged token names a kid we already hold, so it must not trigger rediscovery"
    );
}

#[test]
fn an_unknown_kid_is_the_only_rediscovery_trigger() {
    // The one shape a genuine rotation takes: Keycloak signs with a key whose thumbprint we have
    // never seen. This is the *only* case worth spending a discovery on.
    assert!(
        keys_for_kid(&cached_keys(), "rotated-signing-key").is_none(),
        "a kid outside the cached set must be reported as a miss"
    );
    assert!(
        keys_for_kid(&cached_keys(), KID).is_some(),
        "a kid inside the cached set must resolve"
    );
}

#[test]
fn an_unknown_kid_does_not_fall_back_to_the_other_keys() {
    // The lookup used to return the whole key set on a miss. That destroyed the signal — an
    // unknown `kid` then failed identically to a tampered token — and made each such request cost
    // one public-key operation per cached key.
    let mut keys = cached_keys();
    keys.push(JwkDecodingKey {
        kid: Some(String::from("second-signing-key")),
        key: verification_key(),
    });

    assert!(
        keys_for_kid(&keys, "neither-of-them").is_none(),
        "a miss must stay a miss, never widen to the full key set"
    );
    assert_eq!(
        keys_for_kid(&keys, KID).expect("kid resolves").len(),
        1,
        "a hit must yield only the keys published under that kid"
    );
}

#[tokio::test]
async fn rejects_a_kid_carrying_control_characters() {
    // `kid` is read before any signature is verified, so its content is entirely attacker-chosen,
    // and it flows into log fields from there. A newline lets an unauthenticated caller forge log
    // lines in any plain-text subscriber.
    let mut header = rs256_header();
    header.kid = Some(String::from("real-key\nlevel=INFO msg=\"forged log line\""));
    let token = sign(&header, &base_claims());

    let err = decode_and_validate::<ProfileAndEmail>(&test_instance(), RawToken { token: token.as_str() })
        .await
        .expect_err("a kid with control characters must be rejected");
    let AuthError::InvalidToken { reason } = &err else {
        panic!("expected InvalidToken, got {err:?}");
    };
    assert!(
        !reason.contains('\n'),
        "the rejection must not echo the kid back into its own message: {reason}"
    );
}

#[tokio::test]
async fn rejects_an_overlong_kid() {
    // Unbounded, every rejected request costs as much log volume as the sender chooses. Keycloak's
    // own kid is a ~43 character base64url thumbprint.
    let mut header = rs256_header();
    header.kid = Some("A".repeat(4096));
    let token = sign(&header, &base_claims());

    let err = decode_and_validate::<ProfileAndEmail>(&test_instance(), RawToken { token: token.as_str() })
        .await
        .expect_err("an overlong kid must be rejected");
    assert!(
        matches!(err, AuthError::InvalidToken { .. }),
        "expected InvalidToken, got {err:?}"
    );
}

// ── Role attacks ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_client_role_does_not_satisfy_a_realm_role_check() {
    // Keycloak ships the roles of *every* client the user holds roles on in `resource_access`, and
    // `ExtractRoles` flattens those into the same list as the realm roles. Matching on the bare
    // name would therefore let any other service in the realm grant access to ISM by naming one of
    // its own roles `ADMIN` — no cooperation from this realm's admins required.
    let token = sign(
        &rs256_header(),
        &claims_with(json!({
            "realm_access": { "roles": ["USER"] },
            "resource_access": { "some-other-service": { "roles": ["ADMIN"] } },
        })),
    );

    let err = authenticate_expecting_roles(&token, &policy(), &[AppRole::Admin])
        .await
        .expect_err("a client role must not satisfy a realm role check");
    assert!(
        matches!(err, AuthError::MissingExpectedRole { .. }),
        "expected MissingExpectedRole, got {err:?}"
    );
}

#[tokio::test]
async fn a_realm_role_satisfies_a_realm_role_check() {
    // The control for the test above: without this, that one could pass because role checking is
    // broken outright rather than because the realm/client split works.
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "realm_access": { "roles": ["ADMIN"] } })),
    );

    authenticate_expecting_roles(&token, &policy(), &[AppRole::Admin])
        .await
        .expect("a realm role must satisfy a realm role check");
}

// ── Claim attacks ───────────────────────────────────────────────────────────

#[test]
fn rejects_foreign_issuer() {
    // A token from a Keycloak we do not trust, correctly signed by *that* server.
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "iss": OTHER_ISSUER })),
    );
    let err = verify(&token).expect_err("a foreign issuer must be rejected");

    // Pinned to the exact kind: the issuer is enforced by the `Validation` the policy carries, and
    // a policy built without `set_issuer` would let this token through with nothing else noticing.
    assert!(
        matches!(&err, AuthError::Decode { source } if matches!(source.kind(), ErrorKind::InvalidIssuer)),
        "expected the policy's issuer check to reject this, got {err:?}"
    );
}

#[test]
fn rejects_wrong_audience() {
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "aud": "some-other-service" })),
    );
    verify(&token).expect_err("a foreign audience must be rejected");
}

#[test]
fn rejects_expired_token() {
    // Well beyond `EXPIRY_LEEWAY_SECS`.
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "exp": now() - 3600 })),
    );
    verify(&token).expect_err("an expired token must be rejected");
}

#[tokio::test]
async fn accepts_a_token_expired_within_the_leeway() {
    // Two checks decide expiry — `jsonwebtoken`'s own validation and `assert_not_expired` — and
    // the stricter one wins. This passes only while both apply `EXPIRY_LEEWAY_SECS`, so it is what
    // catches the two drifting apart.
    let token = sign(&rs256_header(), &claims_with(json!({ "exp": now() - 2 })));
    authenticate(&token, &policy())
        .await
        .expect("expiry within the leeway must still authenticate");
}

#[tokio::test]
async fn rejects_a_token_expired_beyond_the_leeway() {
    // Ten seconds is outside the five the leeway grants, but far inside `jsonwebtoken`'s 60-second
    // default — so this fails the moment someone drops the explicit leeway and lets that default
    // back in.
    let token = sign(&rs256_header(), &claims_with(json!({ "exp": now() - 10 })));
    authenticate(&token, &policy())
        .await
        .expect_err("expiry beyond the leeway must be rejected");
}

#[test]
fn expired_token_does_not_look_like_key_rotation() {
    // Ordinary expiry is the single most common auth failure there is, so it must never reach for
    // Keycloak. It cannot: an expired token still names the `kid` it was signed with, we still
    // hold that key, and a known `kid` makes the failure terminal. Note this holds regardless of
    // the `ErrorKind` — the trigger no longer consults it, which is the point. The kind is
    // asserted only to document that expiry is what actually tripped.
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "exp": now() - 3600 })),
    );

    let err = verify(&token).expect_err("an expired token must be rejected");
    let AuthError::Decode { source } = &err else {
        panic!("expected a decode error, got {err:?}");
    };
    assert!(
        matches!(source.kind(), ErrorKind::ExpiredSignature),
        "expected ExpiredSignature, got {:?}",
        source.kind()
    );
    assert!(
        keys_for_kid(&cached_keys(), KID).is_some(),
        "the expired token names a kid we hold, so it must not trigger rediscovery"
    );
}

#[test]
fn rejects_token_not_yet_valid() {
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "nbf": now() + 3600 })),
    );
    verify(&token).expect_err("a not-yet-valid token must be rejected");
}

#[test]
fn rejects_missing_required_claims() {
    // Dropping a claim must not be a way to skip the check that reads it. `exp`/`iss`/`aud`/`sub`
    // are the spec claims `Validation` requires; the rest are ones `StandardClaims` makes
    // non-optional, and since the claim set is deserialised into it before `jsonwebtoken`
    // validates, dropping either kind is caught.
    for claim in ["exp", "iss", "aud", "sub", "iat", "jti", "typ", "azp"] {
        let token = sign(&rs256_header(), &claims_with(json!({ claim: null })));
        assert!(
            verify(&token).is_err(),
            "a token missing '{claim}' must be rejected"
        );
    }
}

#[test]
fn a_claim_set_we_cannot_parse_is_a_401_not_a_500() {
    // Deserialising into `StandardClaims` moved this failure ahead of `jsonwebtoken`'s own claim
    // validation. It must stay a rejected credential: answering 5xx would both leak that the
    // token got past signature verification and turn a realm format change into an error-rate
    // page rather than an auth failure.
    let token = sign(&rs256_header(), &claims_with(json!({ "jti": null })));
    let status = verify(&token)
        .expect_err("an unparsable claim set must be rejected")
        .into_response()
        .status();

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_id_token_replayed_as_access_token() {
    // An ID token is signed by the same realm key and passes every signature and claim check
    // above; only `typ` separates it from a bearer credential.
    let token = sign(&rs256_header(), &claims_with(json!({ "typ": "ID" })));
    let err = authenticate(&token, &policy())
        .await
        .expect_err("an ID token must not authenticate a request");
    assert!(
        matches!(err, AuthError::InvalidToken { .. }),
        "expected the token type to be rejected, got {err:?}"
    );
}

#[tokio::test]
async fn rejects_refresh_token_replayed_as_access_token() {
    let token = sign(&rs256_header(), &claims_with(json!({ "typ": "Refresh" })));
    authenticate(&token, &policy())
        .await
        .expect_err("a refresh token must not authenticate a request");
}

#[tokio::test]
async fn rejects_token_minted_for_another_realm_client() {
    // The reason `aud: "account"` alone is not access control: Keycloak puts that audience on
    // every access token of every client in the realm. Only `azp` distinguishes them.
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "azp": "some-other-client" })),
    );

    authenticate(&token, &policy_pinning_azp())
        .await
        .expect_err("a token from another realm client must be rejected when azp is pinned");

    // Documents the default posture: without `expected_azp` configured, this token is accepted.
    authenticate(&token, &policy())
        .await
        .expect("without azp pinning any realm client is accepted");
}

#[tokio::test]
async fn rejects_non_uuid_subject() {
    let token = sign(
        &rs256_header(),
        &claims_with(json!({ "sub": "../../admin" })),
    );
    let err = authenticate(&token, &policy())
        .await
        .expect_err("a non-UUID subject must be rejected");
    assert!(
        matches!(err, AuthError::InvalidToken { .. }),
        "expected the subject to be rejected, got {err:?}"
    );
}

// ── Structural attacks ──────────────────────────────────────────────────────

#[test]
fn rejects_malformed_tokens() {
    for token in [
        "",
        "not-a-jwt",
        "a.b",
        "a.b.c.d",
        "....",
        &"A".repeat(8192),
        "eyJhbGciOiJSUzI1NiJ9..",
    ] {
        verify(token).expect_err("a malformed token must be rejected");
    }
}

/// A second, unrelated RSA-2048 key, standing in for "signed by someone else".
const FOREIGN_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
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
