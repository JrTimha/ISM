//! The per-request tower `Service` the auth layer produces.
//!
//! `call` is the whole of it, in three steps:
//!
//! 1. Extract the JWT from the request — nothing to extract is an error.
//! 2. Validate it against the realm's signing keys, which may cost one on-demand JWKS refresh.
//! 3. Insert the resulting `KeycloakToken` into the request extensions and call the inner service.
//!
//! A failure in step 1 or 2 is answered here, from `AuthError::into_response`; the request never
//! reaches a handler.

use std::{
    sync::Arc,
    task::{Context, Poll},
};
use crate::auth::error::AuthError;
use crate::auth::layer::KeycloakAuthLayer;
use crate::auth::role::Role;
use axum::{body::Body, response::IntoResponse};
use futures::future::BoxFuture;
use http::Request;
use serde::de::DeserializeOwned;
use tracing::debug;
use crate::auth::extract::extract_jwt;

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

        debug!("Validating incoming request.");
        let clone = self.inner.clone();
        let layer = Arc::clone(&self.layer);
        // Take the service that was ready!
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            //1. extract jwt
            //2. validate token
            //3. add to extensions and call next middleware
            let result = match extract_jwt(&request, &layer.token_extractors) {
                Some(token) => layer.validate_raw_token(&token).await,
                None => Err(AuthError::MissingToken),
            };

            match result {
                Ok(keycloak_token) => {
                    // What every handler behind this layer reads back as `CurrentUser`.
                    request.extensions_mut().insert(keycloak_token);
                    inner.call(request).await
                }
                Err(err) => Ok(err.into_response()),
            }
        })
    }
}
