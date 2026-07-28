//! What the middleware does with a request whose token failed validation.

use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::auth::error::AuthError;
use crate::auth::role::Role;
use crate::auth::token::KeycloakToken;

/// The mode in which the authentication middleware may operate in.
///
/// ```PassthroughMode::Block```: Immediately return a `Response` if authentication failed.
/// On successful authentication, the parsed token content is stored as an axum extension as a `KeycloakToken`.
///
/// ```PassthroughMode::Pass```:  Forward to the response handler regardless of whether there was an authentication failure.
/// In this mode, the authentication status is stored as an axum extension as a `KeycloakAuthStatus`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PassthroughMode {
    Block,
    Pass,
}

/// The authentication result handed to the handler under `PassthroughMode::Pass`.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum KeycloakAuthStatus<R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    // This variant is fairly large, but probably used most of the time. Leaving this non-boxed results in one less allocation each request.
    Success(KeycloakToken<R, Extra>),
    Failure(Arc<AuthError>),
}
