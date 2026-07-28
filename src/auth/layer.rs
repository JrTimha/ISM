//! The tower `Layer` carrying the auth configuration.
//!
//! Built once in `router::init_auth`. `KeycloakAuthService` holds it behind an `Arc`, so nothing
//! in here is copied per request — keep it that way when adding fields.
//!
//! `instance` stays an `Arc` in its own right: a `KeycloakAuthInstance` is meant to be shared
//! across several layers so they do not each run their own OIDC discovery.

use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::{fmt::Debug, sync::Arc};
use tower::Layer;
use typed_builder::TypedBuilder;

use crate::auth::decode::{RawToken, ValidationPolicy, decode_and_validate, parse_raw_claims};
use crate::auth::error::AuthError;
use crate::auth::extract::TokenExtractor;
use crate::auth::token::{KeycloakToken, ProfileAndEmail};
use crate::auth::{instance::KeycloakAuthInstance, role::Role, service::KeycloakAuthService};

use super::PassthroughMode;

/// Add this layer to a router to protect the contained route handlers.
/// Authentication happens by looking for the `Authorization` header on requests and parsing the contained JWT bearer token.
/// See the module level documentation and `docs/auth.md` for how this layer can be created and used.
#[derive(Clone, TypedBuilder)]
pub struct KeycloakAuthLayer<R, Extra = ProfileAndEmail>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    #[builder(setter(into))]
    pub instance: Arc<KeycloakAuthInstance>,

    /// See `PassthroughMode` for more information.
    #[builder(default = PassthroughMode::Block)]
    pub passthrough_mode: PassthroughMode,

    /// Determine if the raw claims extracted from the JWT are persisted as an `Extension`.
    /// If you do not need access to this information, fell free to set this to false.
    #[builder(default = false)]
    pub persist_raw_claims: bool,

    /// Rules incoming tokens are validated against: accepted audiences, authorized parties and
    /// signature algorithms. See `ValidationPolicy`.
    #[builder(setter(into))]
    pub validation_policy: ValidationPolicy,

    /// These roles are always required.
    /// Should a route protected by this layer be accessed by a user not having this role, an error is generated.
    /// If fine-grained role-based access management in required,
    /// leave this empty and perform manual role checks in your route handlers.
    #[builder(default = vec![], setter(into))]
    pub required_roles: Vec<R>,

    /// Specifies where the token is expected to be found. Tried in order, first hit wins.
    ///
    /// An empty list is accepted and fails closed: no extractor yields a token, so every request
    /// is rejected with `AuthError::MissingToken`.
    #[builder(default = vec![Arc::new(crate::auth::extract::AuthHeaderTokenExtractor {})], setter(into))]
    pub token_extractors: Vec<Arc<dyn TokenExtractor>>,

    #[builder(default = uuid::Uuid::now_v7(), setter(skip))]
    id: uuid::Uuid,

    #[builder(default=PhantomData, setter(skip))]
    phantom: PhantomData<Extra>,
}

impl<R, Extra> KeycloakAuthLayer<R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    /// Allows to validate a raw auth token given as &str (without the "Bearer " part when taken from an authorization header).
    /// This method is helpful if you wish to validate a token which does not pass the axum middleware
    /// or if you wish to validate a token in a different context.
    pub async fn validate_raw_token(
        &self,
        raw_token: &str,
    ) -> Result<
        (
            Option<HashMap<String, serde_json::Value>>,
            KeycloakToken<R, Extra>,
        ),
        AuthError,
    > {
        let raw_claims = decode_and_validate(
            self.instance.as_ref(),
            RawToken(raw_token),
            &self.validation_policy,
        )
        .await?;

        parse_raw_claims::<R, Extra>(
            raw_claims,
            self.persist_raw_claims,
            &self.required_roles,
            &self.validation_policy,
        )
        .await
    }
}

impl<R, Extra> Debug for KeycloakAuthLayer<R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeycloakAuthLayer")
            .field("mode", &self.passthrough_mode)
            .field("persist_raw_claims", &self.persist_raw_claims)
            .finish()
    }
}

impl<S, R, Extra> Layer<S> for KeycloakAuthLayer<R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    type Service = KeycloakAuthService<S, R, Extra>;

    #[tracing::instrument(level="info", skip_all, fields(id = ?self.id))]
    fn layer(&self, inner: S) -> Self::Service {
        KeycloakAuthService::new(inner, self)
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use url::Url;

    use crate::auth::{
        AppRole, PassthroughMode,
        decode::ValidationPolicy,
        extract::{AuthHeaderTokenExtractor, QueryParamTokenExtractor, TokenExtractor},
        instance::{KeycloakAuthInstance, KeycloakConfig},
        layer::KeycloakAuthLayer,
    };

    fn test_policy() -> ValidationPolicy {
        ValidationPolicy::new(
            vec![String::from("account")],
            vec![],
            &[String::from("RS256")],
        )
        .expect("valid policy")
    }

    #[tokio::test]
    async fn build_basic_layer() {
        // `without_discovery`, because `new` now insists on reaching Keycloak and this test is
        // about the builder wiring, not about discovery.
        let instance = KeycloakAuthInstance::without_discovery(
            KeycloakConfig::builder()
                .server(Url::parse("https://localhost:8443/").expect("invalid url"))
                .realm(String::from("MyRealm"))
                .build(),
        );

        let _layer = KeycloakAuthLayer::<AppRole>::builder()
            .instance(instance)
            .passthrough_mode(PassthroughMode::Block)
            .validation_policy(test_policy())
            .build();
    }

    #[test]
    fn policy_rejects_symmetric_and_mixed_algorithms() {
        // An `oct` JWK in the key set plus an HS entry here is the RS256 -> HS256 key-confusion
        // setup, so the symmetric family is refused outright.
        assert!(
            ValidationPolicy::new(
                vec![String::from("account")],
                vec![],
                &[String::from("HS256")]
            )
            .is_err()
        );

        // `jsonwebtoken` fails verification when the allow-list spans families; reject it here
        // with an explanation instead of at request time with a blanket 401.
        assert!(
            ValidationPolicy::new(
                vec![String::from("account")],
                vec![],
                &[String::from("RS256"), String::from("ES256")],
            )
            .is_err()
        );

        assert!(ValidationPolicy::new(vec![String::from("account")], vec![], &[]).is_err());
        assert!(
            ValidationPolicy::new(vec![], vec![], &[String::from("RS256")]).is_err(),
            "an empty audience list would disable the audience check"
        );

        // Same family, multiple entries: allowed.
        assert!(
            ValidationPolicy::new(
                vec![String::from("account")],
                vec![],
                &[String::from("RS256"), String::from("PS512")],
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn build_full_layer() {
        let instance = KeycloakAuthInstance::without_discovery(
            KeycloakConfig::builder()
                .server(Url::parse("https://localhost:8443/").expect("invalid url"))
                .realm(String::from("MyRealm"))
                .build(),
        );

        let _layer = KeycloakAuthLayer::<AppRole>::builder()
            .instance(instance)
            .passthrough_mode(PassthroughMode::Block)
            .persist_raw_claims(false)
            .validation_policy(test_policy())
            .required_roles(vec![AppRole::Admin])
            .token_extractors(vec![
                Arc::new(AuthHeaderTokenExtractor::default()) as Arc<dyn TokenExtractor>,
                Arc::new(QueryParamTokenExtractor::default()),
                Arc::new(QueryParamTokenExtractor::extracting_key("jwt")),
            ])
            .build();
    }
}
