//! The per-request tower `Service` the auth layer produces.
//!
//! `poll_ready` holds requests back until the first OIDC discovery has succeeded, so no request
//! is ever rejected merely because the key set had not arrived yet. `call` then extracts and
//! validates the token and, in `PassthroughMode::Block`, either inserts the `KeycloakToken` into
//! the request extensions or answers the error itself.

use std::{
    sync::Arc,
    task::{Context, Poll},
};

use crate::auth::error::AuthError;
use crate::auth::layer::KeycloakAuthLayer;
use crate::auth::role::Role;
use crate::auth::{KeycloakAuthStatus, PassthroughMode, extract};
use axum::{body::Body, response::IntoResponse};
use futures::future::BoxFuture;
use http::Request;
use serde::de::DeserializeOwned;

/// Wraps the inner service with token validation. Created by `KeycloakAuthLayer::layer`.
#[derive(Clone)]
pub struct KeycloakAuthService<S, R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    inner: S,
    /// Shared rather than owned: tower clones the whole service per request, and `call` needs a
    /// `'static` handle to move into the response future. Both are then a single refcount bump
    /// instead of a structural copy of every field the layer holds.
    layer: Arc<KeycloakAuthLayer<R, Extra>>,
}

impl<S, R, Extra> KeycloakAuthService<S, R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    /// Clones the layer exactly once, when the router is built — never per request.
    pub fn new(inner: S, layer: &KeycloakAuthLayer<R, Extra>) -> Self {
        Self {
            inner,
            layer: Arc::new(layer.clone()),
        }
    }
}

impl<S, R, Extra> tower::Service<Request<Body>> for KeycloakAuthService<S, R, Extra>
where
    S: tower::Service<Request<Body>, Response = axum::response::Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    R: Role + 'static,
    Extra: DeserializeOwned + Clone + Sync + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // No discovery gate here: `KeycloakAuthInstance::new` does not return until one discovery
        // has succeeded, and a later failed refresh keeps the keys it already had. There is
        // therefore no state in which this service exists without a usable key set to wait for.
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<Body>) -> Self::Future {
        tracing::debug!("Validating request...");

        let clone = self.inner.clone();
        let layer = Arc::clone(&self.layer);

        // Take the service that was ready!
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            // Process the request.
            let result = match extract::extract_jwt(&request, &layer.token_extractors) {
                Some(token) => layer.validate_raw_token(&token).await,
                None => Err(AuthError::MissingToken),
            };

            match result {
                Ok((raw_claims, keycloak_token)) => {
                    if let Some(raw_claims) = raw_claims {
                        request.extensions_mut().insert(raw_claims);
                    }
                    match layer.passthrough_mode {
                        PassthroughMode::Block => {
                            request.extensions_mut().insert(keycloak_token);
                        }
                        PassthroughMode::Pass => {
                            request
                                .extensions_mut()
                                .insert(KeycloakAuthStatus::<R, Extra>::Success(keycloak_token));
                        }
                    };
                    inner.call(request).await
                }
                Err(err) => match layer.passthrough_mode {
                    PassthroughMode::Block => Ok(err.into_response()),
                    PassthroughMode::Pass => {
                        request
                            .extensions_mut()
                            .insert(KeycloakAuthStatus::<R, Extra>::Failure(Arc::new(err)));
                        inner.call(request).await
                    }
                },
            }
        })
    }
}
