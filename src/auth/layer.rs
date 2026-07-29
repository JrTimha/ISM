//! The tower `Layer` carrying the auth configuration.
//!
//! Built once in `router::init_auth`. `KeycloakAuthService` holds it behind an `Arc`, so nothing
//! in here is copied per request — keep it that way when adding fields.
//!
//! `instance` stays an `Arc` in its own right: a `KeycloakAuthInstance` is meant to be shared
//! across several layers so they do not each run their own OIDC discovery. It also carries the
//! `ValidationPolicy`, which is why nothing in here describes what a valid token looks like — this
//! layer only decides what is additionally *required* of one, and where to find it on the request.

use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::{fmt::Debug, sync::Arc};
use tower::Layer;
use typed_builder::TypedBuilder;

use crate::auth::decode::{RawToken, decode_and_validate, parse_claims};
use crate::auth::error::AuthError;
use crate::auth::extract::TokenExtractor;
use crate::auth::token::{KeycloakToken, ProfileAndEmail};
use crate::auth::{instance::KeycloakAuthInstance, role::Role, service::KeycloakAuthService};

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
    ) -> Result<KeycloakToken<R, Extra>, AuthError> {
        let claims = decode_and_validate::<Extra>(
            self.instance.as_ref(),
            RawToken { token: raw_token },
        )
        .await?;

        parse_claims::<R, Extra>(claims, &self.required_roles, self.instance.get_jwt_validation_policy())
    }
}

impl<R, Extra> Debug for KeycloakAuthLayer<R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeycloakAuthLayer")
            .field("required_roles", &self.required_roles.len())
            .field("token_extractors", &self.token_extractors.len())
            .finish_non_exhaustive()
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
        AppRole, ValidationPolicy,
        extract::{AuthHeaderTokenExtractor, QueryParamTokenExtractor, TokenExtractor},
        instance::{KeycloakAuthInstance, KeycloakConfig},
        layer::KeycloakAuthLayer,
    };

    /// Stands in for what OIDC discovery would report, which is where the real issuer comes from.
    const ISSUER: &str = "https://localhost:8443/realms/MyRealm";

    /// `without_discovery`, because `new` insists on reaching Keycloak and these tests are about
    /// the builder wiring, not about discovery. That also means the policy has to be handed over
    /// rather than derived from a discovered issuer.
    fn test_instance() -> KeycloakAuthInstance {
        let policy = ValidationPolicy::new(
            ISSUER.to_owned(),
            vec![String::from("account")],
            vec![],
            &[String::from("RS256")],
        )
        .expect("valid policy");

        KeycloakAuthInstance::without_discovery(
            KeycloakConfig::builder()
                .server(Url::parse("https://localhost:8443/").expect("invalid url"))
                .realm(String::from("MyRealm"))
                .expected_audiences(vec![String::from("account")])
                .expected_azp(vec![])
                .allowed_algorithms(vec![String::from("RS256")])
                .build(),
            policy,
        )
    }

    #[tokio::test]
    async fn build_basic_layer() {
        let _layer = KeycloakAuthLayer::<AppRole>::builder()
            .instance(test_instance())
            .build();
    }

    #[tokio::test]
    async fn build_full_layer() {
        let _layer = KeycloakAuthLayer::<AppRole>::builder()
            .instance(test_instance())
            .required_roles(vec![AppRole::Admin])
            .token_extractors(vec![
                Arc::new(AuthHeaderTokenExtractor::default()) as Arc<dyn TokenExtractor>,
                Arc::new(QueryParamTokenExtractor::default()),
                Arc::new(QueryParamTokenExtractor::extracting_key("jwt")),
            ])
            .build();
    }
}
