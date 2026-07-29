//! The rules incoming tokens are validated against.
//!
//! Built once, by `KeycloakAuthInstance::new`, and never rebuilt per request. It belongs to the
//! instance rather than to a layer because it describes one realm: the same discovery that
//! produces the signing keys produces the issuer pinned here, and an instance is meant to be
//! shared across layers. Two layers over one realm therefore cannot disagree about what a valid
//! token looks like.
//!
//! Everything in here is fixed at startup and independent of anything a caller controls — see
//! `RawToken::decode_and_validate`. Every rule has a matching attack in `security_tests.rs`.

use jsonwebtoken::{Algorithm, AlgorithmFamily, Validation};
use std::str::FromStr;

use crate::auth::error::AuthError;

/// How far past its `exp` a token is still accepted, absorbing clock drift between the Keycloak
/// host and this one.
///
/// Applied in two places that both run on every request: `jsonwebtoken`'s own `exp`/`nbf`
/// validation, seeded below, and the explicit `assert_not_expired` in `decode.rs`. The stricter of
/// the two decides, so they must agree — this constant is what makes them agree. It deliberately
/// replaces `jsonwebtoken`'s 60-second default, which was previously dead anyway because the
/// explicit check ran with no leeway at all.
pub const EXPIRY_LEEWAY_SECS: i64 = 5;

/// The rules incoming tokens are validated against, fixed at startup.
#[derive(Debug, Clone)]
pub struct ValidationPolicy {
    /// The single accepted `iss`, taken from the realm's discovery document rather than rebuilt
    /// from `iss_host`/`iss_realm` — a configured frontend URL makes those two differ.
    pub expected_issuer: String,
    /// Accepted `aud` values.
    pub expected_audiences: Vec<String>,
    /// Accepted `azp` values. Empty disables the check.
    pub expected_azp: Vec<String>,
    /// Accepted signature algorithms.
    pub allowed_algorithms: Vec<Algorithm>,
    /// Built once here and reused for every token, rather than reassembled per request — it costs
    /// several hash sets and string allocations. Covers every rule `jsonwebtoken` can enforce
    /// itself, the issuer included.
    validation: Validation,
}

impl ValidationPolicy {
    /// Builds a policy, rejecting unusable configuration up front so a mistake surfaces at startup
    /// rather than as a blanket 401 at runtime.
    ///
    /// `expected_issuer` comes from OIDC discovery, which is why `KeycloakAuthInstance::new` calls
    /// this only after its first discovery has succeeded. Taking it as a parameter rather than
    /// applying it afterwards is what makes a policy without issuer validation unrepresentable.
    pub fn new(
        expected_issuer: String,
        expected_audiences: Vec<String>,
        expected_azp: Vec<String>,
        algorithm_names: &[String],
    ) -> Result<Self, String> {
        let allowed_algorithms = algorithm_names
            .iter()
            .map(|name| {
                Algorithm::from_str(name).map_err(|_| format!("unknown signature algorithm: {name}"))
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

        if expected_issuer.is_empty() {
            return Err("expected_issuer must not be empty".to_owned());
        }

        // The algorithm allow-list comes from configuration, never from `header.alg`. Deriving it
        // from the header lets the caller choose the family their token is verified under, which
        // is the setup for RS256 -> HS256 key-confusion forgery.
        let mut validation = Validation::new_for_family(first.family());
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.validate_aud = true;
        validation.leeway = EXPIRY_LEEWAY_SECS as u64; //stricter than the default
        validation.algorithms = allowed_algorithms.clone();
        validation.set_audience(&expected_audiences);
        validation.set_issuer(&[&expected_issuer]);

        // `iss` has to be listed here for the line above to bite: `jsonwebtoken` validates the
        // issuer only when the claim is present, so without this a token that simply omits `iss`
        // would skip the check entirely rather than fail it.
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        Ok(Self {
            expected_issuer,
            expected_audiences,
            expected_azp,
            allowed_algorithms,
            validation,
        })
    }

    /// The rule set handed to `jsonwebtoken` on every request.
    pub(super) fn validation(&self) -> &Validation {
        &self.validation
    }

    /// Rejects tokens minted for a different Keycloak client in the same realm.
    pub(super) fn assert_authorized_party(&self, azp: &str) -> Result<(), AuthError> {
        if self.expected_azp.is_empty() || self.expected_azp.iter().any(|it| it == azp) {
            return Ok(());
        }
        Err(AuthError::InvalidToken {
            reason: format!("unexpected authorized party: {azp}"),
        })
    }
}

#[cfg(test)]
mod test {
    use super::ValidationPolicy;

    const ISSUER: &str = "https://localhost:8443/realms/MyRealm";

    fn policy_with(audiences: Vec<String>, algorithms: &[String]) -> Result<ValidationPolicy, String> {
        ValidationPolicy::new(ISSUER.to_owned(), audiences, vec![], algorithms)
    }

    #[test]
    fn rejects_symmetric_and_mixed_algorithm_families() {
        // An `oct` JWK in the key set plus an HS entry here is the RS256 -> HS256 key-confusion
        // setup, so the symmetric family is refused outright.
        assert!(policy_with(vec![String::from("account")], &[String::from("HS256")]).is_err());

        // `jsonwebtoken` fails verification when the allow-list spans families; reject it here
        // with an explanation instead of at request time with a blanket 401.
        assert!(
            policy_with(
                vec![String::from("account")],
                &[String::from("RS256"), String::from("ES256")]
            )
            .is_err()
        );

        assert!(policy_with(vec![String::from("account")], &[]).is_err());

        // Same family, multiple entries: allowed.
        assert!(
            policy_with(
                vec![String::from("account")],
                &[String::from("RS256"), String::from("PS512")]
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_configuration_that_would_disable_a_check() {
        assert!(
            policy_with(vec![], &[String::from("RS256")]).is_err(),
            "an empty audience list would disable the audience check"
        );
        assert!(
            ValidationPolicy::new(
                String::new(),
                vec![String::from("account")],
                vec![],
                &[String::from("RS256")]
            )
            .is_err(),
            "an empty issuer would disable the issuer check"
        );
    }
}
