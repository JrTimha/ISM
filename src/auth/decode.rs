//! Signature and claim validation.
//!
//! `ValidationPolicy` fixes the rules at startup from configuration; `decode_and_validate` runs
//! them against the keys cached in the `KeycloakAuthInstance` and hands back a `KeycloakToken`
//! (defined in `token.rs`). Every rule in here has a matching attack in `security_tests.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::error::AuthError;
use crate::auth::instance::{KeycloakAuthInstance, keys_for_kid};
use crate::auth::role::{ExpectRoles, Role};
use crate::auth::token::{KeycloakToken, StandardClaims};
use chrono::TimeDelta;
use jsonwebtoken::{Algorithm, AlgorithmFamily, DecodingKey, Header, Validation, decode};
use serde::de::DeserializeOwned;
use std::str::FromStr;
use tracing::debug;

pub type RawClaims = HashMap<String, serde_json::Value>;

type DecodedTokenResult = Result<jsonwebtoken::TokenData<HashMap<String, serde_json::Value>>, AuthError>;

/// Token type Keycloak stamps onto access tokens. ID and refresh tokens carry a different `typ`
/// but are signed by the same realm key, so this claim is what separates them.
const ACCESS_TOKEN_TYP: &str = "Bearer";

/// How far past its `exp` a token is still accepted, absorbing clock drift between the Keycloak
/// host and this one.
///
/// Applied in two places that both run on every request: `jsonwebtoken`'s own `exp`/`nbf`
/// validation below, and the explicit `assert_not_expired` in `decode_and_validate`. The stricter
/// of the two decides, so they must agree — this constant is what makes them agree. It deliberately
/// replaces `jsonwebtoken`'s 60-second default, which was previously dead anyway because the
/// explicit check ran with no leeway at all.
pub const EXPIRY_LEEWAY_SECS: i64 = 5;

/// Longest `kid` accepted from a token header.
///
/// Keycloak publishes a base64url thumbprint of around 43 characters. The limit is generous next to
/// that and exists only to bound what an unauthenticated caller can push into a log line — `kid` is
/// read before any signature is verified, so its content is entirely attacker-chosen.
const MAX_KID_LEN: usize = 256;

/// The rules incoming tokens are validated against, fixed at startup from configuration.
///
/// Deliberately independent of anything the caller controls — see `RawToken::decode_and_validate`.
#[derive(Debug, Clone)]
pub struct ValidationPolicy {
    /// Accepted `aud` values.
    pub expected_audiences: Vec<String>,
    /// Accepted `azp` values. Empty disables the check.
    pub expected_azp: Vec<String>,
    /// Accepted signature algorithms.
    pub allowed_algorithms: Vec<Algorithm>,
    /// Built once here and reused for every token, rather than reassembled per request — it costs
    /// several hash sets and string allocations. Covers everything fixed at startup; the issuer is
    /// checked separately in `decode_and_validate`, since it only becomes known through discovery.
    validation: Validation,
}

impl ValidationPolicy {
    /// Builds a policy from configuration, rejecting unusable algorithm lists up front so a
    /// misconfiguration surfaces at startup rather than as a blanket 401 at runtime.
    pub fn new(
        expected_audiences: Vec<String>,
        expected_azp: Vec<String>,
        algorithm_names: &[String],
    ) -> Result<Self, String> {
        let allowed_algorithms = algorithm_names
            .iter()
            .map(|name| {
                Algorithm::from_str(name)
                    .map_err(|_| format!("unknown signature algorithm: {name}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let Some(first) = allowed_algorithms.first().copied() else {
            return Err("allowed_algorithms must not be empty".to_owned());
        };

        if first.family() == AlgorithmFamily::Hmac {
            return Err(
                "allowed_algorithms must not contain a symmetric (HS*) algorithm: Keycloak signs \
                 with asymmetric keys, and accepting HMAC invites key-confusion forgery"
                    .to_owned(),
            );
        }

        // `jsonwebtoken` rejects verification outright when the allow-list spans more than one
        // family, so catch that here where we can explain it.
        if let Some(mismatch) = allowed_algorithms.iter().find(|alg| alg.family() != first.family()) {
            return Err(format!(
                "allowed_algorithms must all belong to the same family, but {first:?} and \
                 {mismatch:?} do not"
            ));
        }

        if expected_audiences.is_empty() {
            return Err("expected_audiences must not be empty".to_owned());
        }

        // The algorithm allow-list comes from configuration, never from `header.alg`. Deriving it
        // from the header lets the caller choose the family their token is verified under, which
        // is the setup for RS256 -> HS256 key-confusion forgery.
        let mut validation = Validation::new_for_family(first.family());
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = EXPIRY_LEEWAY_SECS as u64;
        validation.algorithms = allowed_algorithms.clone();
        validation.set_audience(&expected_audiences);

        // `iss` is required here even though it is compared by hand below, so a token that omits
        // it is rejected before the comparison ever runs.
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        Ok(Self {
            expected_audiences,
            expected_azp,
            allowed_algorithms,
            validation,
        })
    }

    /// Rejects tokens minted for a different Keycloak client in the same realm.
    fn assert_authorized_party(&self, azp: &str) -> Result<(), AuthError> {
        if self.expected_azp.is_empty() || self.expected_azp.iter().any(|it| it == azp) {
            return Ok(());
        }
        Err(AuthError::InvalidToken {
            reason: format!("unexpected authorized party: {azp}"),
        })
    }
}

/// A bearer token as it came off the wire, before any validation.
pub struct RawToken<'a>(pub &'a str);

impl RawToken<'_> {
    pub fn decode_header(&self) -> Result<Header, AuthError> {
        let jwt_header = jsonwebtoken::decode_header(self.0)
            .map_err(|source| AuthError::DecodeHeader { source })?;
        debug!(?jwt_header, "Decoded JWT header");
        Ok(jwt_header)
    }

    /// Verifies the signature against `decoding_keys` and every claim rule in `policy`.
    pub fn decode_and_validate(
        &self,
        policy: &ValidationPolicy,
        issuer: &str,
        decoding_keys: &[&DecodingKey],
    ) -> Result<RawClaims, AuthError> {
        let mut token_data: DecodedTokenResult = Err(AuthError::NoDecodingKeys);

        for key in decoding_keys {
            token_data = decode::<RawClaims>(self.0, key, &policy.validation)
                .map_err(|source| AuthError::Decode { source });
            if token_data.is_ok() {
                break;
            }
        }
        let token_data = token_data?;
        let raw_claims = token_data.claims;

        // Pinned here rather than through `Validation::set_issuer`, because the expected issuer
        // only becomes known through OIDC discovery: folding it into the policy's `Validation`
        // would force that whole struct — several hash sets and string allocations — to be rebuilt
        // on every request. This is the same set-membership test `jsonwebtoken` performs, over a
        // set of one.
        let token_issuer = raw_claims
            .get("iss")
            .and_then(|it| it.as_str())
            .unwrap_or_default();
        if token_issuer != issuer {
            return Err(AuthError::InvalidToken {
                reason: format!("unexpected issuer: {token_issuer}"),
            });
        }

        // Only the subject is logged. The full claim set carries the user's email, username and
        // roles, and this runs at an operator-settable log level.
        debug!(sub = ?raw_claims.get("sub"), "Decoded JWT claims");
        Ok(raw_claims)
    }
}

/// Validates a token against the currently discovered keys, re-running discovery once if the
/// token names a signing key we do not know.
///
/// The refresh is gated on the header's `kid`, never on the outcome of the signature check. That
/// check cannot answer the question: `ErrorKind::InvalidSignature` has a single construction site
/// in `jsonwebtoken` and means only "the verify operation returned false", so a payload-tampered
/// token and a token signed by a rotated key produce the identical error. Keying off it therefore
/// let any invalid token — every expired session, every replayed token — reach for Keycloak.
///
/// The `kid` does answer it. Keycloak stamps a per-key thumbprint on every access token and does
/// not reuse it across rotations, so:
///
/// - **known `kid`** — we hold the exact key the token names. Whatever fails now is a property of
///   the token, not of our key set; rediscovery cannot change the outcome. Terminal.
/// - **unknown `kid`** — the only shape a rotation can take. Worth one refresh.
/// - **no `kid`** — nothing to match a key against, and not something Keycloak issues. Rejected
///   before any signature verification runs.
pub async fn decode_and_validate(
    kc_instance: &KeycloakAuthInstance,
    raw_token: RawToken<'_>,
    policy: &ValidationPolicy,
) -> Result<RawClaims, AuthError> {
    let header = raw_token.decode_header()?;

    let Some(kid) = header.kid.as_deref() else {
        return Err(AuthError::InvalidToken {
            reason: "token header names no signing key (kid)".to_owned(),
        });
    };

    // `kid` reaches this point unauthenticated — it is read before any signature is verified — and
    // from here it flows into log fields and into `UnknownSigningKey`. Unchecked, a `kid` carrying
    // newlines or control characters can forge log lines, and an arbitrarily long one turns every
    // rejected request into as much log volume as the sender cares to send. Keycloak's own `kid` is
    // a base64url thumbprint, so printable ASCII with a length bound loses nothing.
    //
    // The rejection deliberately does not echo the `kid` back into its own message.
    if kid.len() > MAX_KID_LEN || !kid.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(AuthError::InvalidToken {
            reason: "token header names a malformed signing key (kid)".to_owned(),
        });
    }

    async fn try_decode(
        kc_instance: &KeycloakAuthInstance,
        kid: &str,
        raw_token: &RawToken<'_>,
        policy: &ValidationPolicy,
    ) -> Result<RawClaims, AuthError> {
        let discovered = kc_instance.discovered().await;
        // The issuer we pin against is the one Keycloak advertises in its discovery document,
        // not one rebuilt from `iss_host`/`iss_realm` — a configured frontend URL makes those
        // two differ.
        let issuer = discovered.issuer().ok_or(AuthError::NoOidcDiscovery)?;
        let Some(keys) = keys_for_kid(discovered.decoding_keys(), kid) else {
            return Err(AuthError::UnknownSigningKey {
                kid: kid.to_owned(),
            });
        };
        raw_token.decode_and_validate(policy, issuer, &keys)
    }

    // First decode. A separate function so the read guard it holds is released here on return:
    // installing a refreshed key set below needs a write guard on that same lock, and Tokio's
    // `RwLock` is write-preferring, so holding the read guard across it would deadlock.
    let raw_claims = try_decode(kc_instance, kid, &raw_token, policy).await;

    // Only an unknown `kid` is worth re-running discovery for. Every other failure came from a key
    // we hold, which makes it a property of the token rather than of our key set — rediscovery
    // cannot change the verdict, and reaching for Keycloak on each one meant every expired session
    // did so too.
    if !matches!(raw_claims, Err(AuthError::UnknownSigningKey { .. })) {
        return raw_claims;
    }

    // Second decode, against a freshly discovered key set. Either Keycloak rotated its signing key
    // or someone made a `kid` up; both look the same from here, so the refresh is rate-limited and
    // time-boxed rather than trusted — see `KeycloakAuthInstance::refresh_for_request`. If the
    // `kid` is still unknown afterwards, this returns `UnknownSigningKey` again without spending a
    // single public-key operation on it.
    debug!(kid,"Token names an unknown signing key. Re-running discovery.");
    kc_instance.refresh_for_request().await;
    try_decode(kc_instance, kid, &raw_token, policy).await
}

/// Turns a validated claim map into a `KeycloakToken`, applying the checks that need the parsed
/// claims: token type, expiry, authorized party and the layer's required roles.
pub async fn parse_raw_claims<R, Extra>(
    raw_claims: RawClaims,
    persist_raw_claims: bool,
    required_roles: &[R],
    policy: &ValidationPolicy,
) -> Result<
    (
        Option<HashMap<String, serde_json::Value>>,
        KeycloakToken<R, Extra>,
    ),
    AuthError,
>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    let raw_claims_clone = match persist_raw_claims {
        true => Some(raw_claims.clone()),
        false => None,
    };
    let value = serde_json::Value::from_iter(raw_claims);

    let standard_claims: StandardClaims<Extra> =
        serde_json::from_value(value).map_err(|err| AuthError::JsonParse {
            source: Arc::new(err),
        })?;

    // Reject anything that is not an access token. ID and refresh tokens are signed by the same
    // realm key and pass every check above, so without this they could be replayed as bearer
    // credentials.
    if standard_claims.typ != ACCESS_TOKEN_TYP {
        return Err(AuthError::InvalidToken {
            reason: format!("unexpected token type: {}", standard_claims.typ),
        });
    }

    let keycloak_token = KeycloakToken::<R, Extra>::parse(standard_claims)?;
    keycloak_token.assert_not_expired(TimeDelta::seconds(EXPIRY_LEEWAY_SECS))?;
    policy.assert_authorized_party(&keycloak_token.authorized_party)?;
    keycloak_token.expect_roles(required_roles)?;
    Ok((raw_claims_clone, keycloak_token))
}
