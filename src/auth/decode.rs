//! Signature and claim validation.
//!
//! Runs the rules the `KeycloakAuthInstance` carries as its `ValidationPolicy` (see `policy.rs`)
//! against the keys that same instance cached, and hands back the claims a `KeycloakToken` is
//! parsed from (see `token.rs`). Every rule applied here has a matching attack in
//! `security_tests.rs`.

use crate::auth::error::AuthError;
use crate::auth::instance::{KeycloakAuthInstance, keys_for_kid};
use crate::auth::policy::{EXPIRY_LEEWAY_SECS, ValidationPolicy};
use crate::auth::role::{ExpectRoles, Role};
use crate::auth::token::{KeycloakToken, StandardClaims};
use chrono::TimeDelta;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{DecodingKey, Header, decode};
use serde::de::DeserializeOwned;
use tracing::debug;

/// Token type Keycloak stamps onto access tokens. ID and refresh tokens carry a different `typ`
/// but are signed by the same realm key, so this claim is what separates them.
const ACCESS_TOKEN_TYP: &str = "Bearer";

/// Longest `kid` accepted from a token header.
///
/// Keycloak publishes a base64url thumbprint of around 43 characters. The limit is generous next to
/// that and exists only to bound what an unauthenticated caller can push into a log line — `kid` is
/// read before any signature is verified, so its content is entirely attacker-chosen.
const MAX_KID_LEN: usize = 256;

/// A bearer token as it came off the wire, before any validation.
pub struct RawToken<'a>{
    pub token: &'a str
}

impl RawToken<'_> {
    
    pub fn decode_header(&self) -> Result<Header, AuthError> {
        let jwt_header = jsonwebtoken::decode_header(self.token)
            .map_err(|source| AuthError::DecodeHeader { source })?;
        Ok(jwt_header)
    }

    /// Verifies the signature against `decoding_keys` and every claim rule in `policy`.
    ///
    /// Deserialises straight into `StandardClaims` rather than into an intermediate claim map:
    /// `jsonwebtoken` verifies the signature before it hands the payload to serde, so the target
    /// type costs nothing in safety and going through a `HashMap` only meant parsing the same
    /// payload again on the way back out.
    pub fn decode_and_validate<Extra>(
        &self,
        policy: &ValidationPolicy,
        decoding_keys: &[&DecodingKey],
    ) -> Result<StandardClaims<Extra>, AuthError>
    where
        Extra: DeserializeOwned,
    {
        let mut result = Err(AuthError::NoDecodingKeys);

        for key in decoding_keys {
            result = decode::<StandardClaims<Extra>>(self.token, key, policy.validation())
                .map(|token_data| token_data.claims)
                .map_err(classify_decode_error);

            // A wrong key is the only thing the next key can fix. Every other outcome is settled
            // by the token itself, and carrying on past one would report whatever the *last* key
            // happened to complain about instead of the reason the token was actually rejected.
            if !is_signature_failure(&result) {
                break;
            }
        }

        let claims = result?;

        // Only the subject is logged. The full claim set carries the user's email, username and
        // roles, and this runs at an operator-settable log level.
        debug!(sub = %claims.sub, "Decoded JWT claims");
        Ok(claims)
    }
}

/// Every way `decode` can fail is a rejected credential, so they all collapse into one 401.
///
/// The claim set is deserialised before `jsonwebtoken` validates it, which means a token missing
/// `exp`/`iss`/`aud`/`sub` now surfaces as a serde error rather than `MissingRequiredClaim` — the
/// same rejection either way, and deliberately indistinguishable to the caller.
///
/// A serde error is worth a word server-side though. It is only reachable once the signature has
/// verified, so no caller can provoke it: it means the realm started issuing a claim set
/// `StandardClaims` does not describe, which would otherwise fail every request in silence.
fn classify_decode_error(source: jsonwebtoken::errors::Error) -> AuthError {
    if let ErrorKind::Json(err) = source.kind() {
        tracing::warn!(
            error = %err,
            "A signature-valid token did not match StandardClaims. If this is not isolated, the \
             realm is issuing claims this service cannot parse."
        );
    }
    AuthError::Decode { source }
}

/// Whether the outcome is "this key did not sign this token" — the one failure another key can fix.
fn is_signature_failure<T>(result: &Result<T, AuthError>) -> bool {
    matches!(
        result,
        Err(AuthError::Decode { source }) if matches!(source.kind(), ErrorKind::InvalidSignature)
    )
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
pub async fn decode_and_validate<Extra>(
    kc_instance: &KeycloakAuthInstance,
    raw_token: RawToken<'_>,
) -> Result<StandardClaims<Extra>, AuthError>
where
    Extra: DeserializeOwned,
{
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

    fn try_decode<Extra>(
        kc_instance: &KeycloakAuthInstance,
        kid: &str,
        raw_token: &RawToken<'_>,
    ) -> Result<StandardClaims<Extra>, AuthError>
    where
        Extra: DeserializeOwned,
    {
        // An owned snapshot of the current discovery, so nothing on the instance stays locked
        // while the signature below is verified — and so the refresh further down is free to
        // install a new key set at any point.
        let discovered = kc_instance.get_discovered_jwks();
        let Some(keys) = keys_for_kid(&discovered.decoding_keys, kid) else {
            return Err(AuthError::UnknownSigningKey {
                kid: kid.to_owned(),
            });
        };
        raw_token.decode_and_validate(kc_instance.get_jwt_validation_policy(), &keys)
    }

    // First decode, against the key set we currently hold.
    let claims = try_decode::<Extra>(kc_instance, kid, &raw_token);

    // Only an unknown `kid` is worth re-running discovery for. Every other failure came from a key
    // we hold, which makes it a property of the token rather than of our key set — rediscovery
    // cannot change the verdict, and reaching for Keycloak on each one meant every expired session
    // did so too.
    if !matches!(claims, Err(AuthError::UnknownSigningKey { .. })) {
        return claims;
    }

    // Second decode, against a freshly discovered key set. Either Keycloak rotated its signing key
    // or someone made a `kid` up; both look the same from here, so the refresh is rate-limited and
    // time-boxed rather than trusted — see `KeycloakAuthInstance::refresh_jwks_for_request`. If the
    // `kid` is still unknown afterwards, this returns `UnknownSigningKey` again without spending a
    // single public-key operation on it.
    debug!(kid, "Token names an unknown signing key. Re-running discovery.");
    kc_instance.refresh_jwks_for_request().await;
    try_decode(kc_instance, kid, &raw_token)
}

/// Applies the checks that need the parsed claims, then turns them into a `KeycloakToken`: token
/// type, expiry, authorized party and the layer's required roles.
pub fn parse_claims<R, Extra>(
    standard_claims: StandardClaims<Extra>,
    required_roles: &[R],
    policy: &ValidationPolicy,
) -> Result<KeycloakToken<R, Extra>, AuthError>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
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
    Ok(keycloak_token)
}
