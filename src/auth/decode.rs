//! Signature and claim validation.
//!
//! `ValidationPolicy` fixes the rules at startup from configuration; `decode_and_validate` runs
//! them against the keys cached in the `KeycloakAuthInstance` and hands back a `KeycloakToken`
//! (defined in `token.rs`). Every rule in here has a matching attack in `security_tests.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::error::AuthError;
use crate::auth::error::DecodeHeaderSnafu;
use crate::auth::error::DecodeSnafu;
use crate::auth::instance::KeycloakAuthInstance;
use crate::auth::role::{ExpectRoles, Role};
use crate::auth::token::{KeycloakToken, StandardClaims};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, AlgorithmFamily, DecodingKey, Header, Validation, decode};
use serde::de::DeserializeOwned;
use snafu::ResultExt;
use std::str::FromStr;
use tracing::debug;

pub type RawClaims = HashMap<String, serde_json::Value>;

type DecodedTokenResult =
    Result<jsonwebtoken::TokenData<HashMap<String, serde_json::Value>>, AuthError>;

/// Token type Keycloak stamps onto access tokens. ID and refresh tokens carry a different `typ`
/// but are signed by the same realm key, so this claim is what separates them.
const ACCESS_TOKEN_TYP: &str = "Bearer";

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
        if let Some(mismatch) = allowed_algorithms
            .iter()
            .find(|alg| alg.family() != first.family())
        {
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
        let mut validation = jsonwebtoken::Validation::new_for_family(first.family());
        validation.algorithms = allowed_algorithms.clone();
        validation.set_audience(&expected_audiences);
        validation.validate_nbf = true;
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
        let jwt_header = jsonwebtoken::decode_header(self.0).context(DecodeHeaderSnafu {})?;
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
            token_data =
                decode::<RawClaims>(self.0, key, &policy.validation).context(DecodeSnafu {});
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
/// failure looks like a signing-key rotation.
pub async fn decode_and_validate(
    kc_instance: &KeycloakAuthInstance,
    raw_token: RawToken<'_>,
    policy: &ValidationPolicy,
) -> Result<RawClaims, AuthError> {
    let header = raw_token.decode_header()?;

    async fn try_decode(
        kc_instance: &KeycloakAuthInstance,
        header: &Header,
        raw_token: &RawToken<'_>,
        policy: &ValidationPolicy,
    ) -> Result<RawClaims, AuthError> {
        let discovered = kc_instance.discovered().await;
        // The issuer we pin against is the one Keycloak advertises in its discovery document,
        // not one rebuilt from `iss_host`/`iss_realm` — a configured frontend URL makes those
        // two differ.
        let issuer = discovered.issuer().ok_or(AuthError::NoOidcDiscovery)?;
        let keys = discovered.candidate_keys(header.kid.as_deref());
        raw_token.decode_and_validate(policy, issuer, &keys)
    }

    // First decode. This may fail if known decoding keys are out of date (for example if the Keycloak server changed).
    let mut raw_claims = try_decode(kc_instance, &header, &raw_token, policy).await;

    if raw_claims.is_err() {
        // If it makes sense to do so, refresh the decoding keys through a new discovery process
        // and try to decode again.
        // This may delay handling of the request in flight by a non-marginal amount of time
        // but may allow us to acknowledge it in the end without rejecting the call immediately,
        // which would then (probably) require a retry from our caller anyway!
        //
        // `perform_oidc_discovery` is rate-limited, so a flood of unverifiable tokens cannot turn
        // this branch into a request storm against Keycloak.
        let retry = match raw_claims.as_ref() {
            // Discovery has not produced anything usable yet.
            Err(AuthError::NoOidcDiscovery) | Err(AuthError::NoDecodingKeys) => true,
            Err(AuthError::Decode { source }) => matches!(
                source.kind(),
                // The signature did not verify under any key we hold. This is exactly what a
                // Keycloak signing-key rotation looks like, and it is the only error kind that
                // rotation produces — `InvalidSignature` has a single construction site in
                // `jsonwebtoken`, in `verify_signature_body`. Note this branch previously listed
                // `RsaFailedSigning` for this case, which is a signing-side kind that the crate
                // never constructs at all; rotation therefore never triggered a refresh and the
                // service stayed hard-down on auth until it was restarted.
                ErrorKind::InvalidSignature
                    // Unusable key material in the cached JWKS; a re-fetch may yield a good key.
                    | ErrorKind::InvalidRsaKey(_)
                    | ErrorKind::InvalidEcdsaKey
            ),
            _ => false,
        };

        // Second decode
        if retry {
            kc_instance.perform_oidc_discovery().await;
            raw_claims = try_decode(kc_instance, &header, &raw_token, policy).await;
        }
    }

    raw_claims
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
    keycloak_token.assert_not_expired()?;
    policy.assert_authorized_party(&keycloak_token.authorized_party)?;
    keycloak_token.expect_roles(required_roles)?;
    Ok((raw_claims_clone, keycloak_token))
}
