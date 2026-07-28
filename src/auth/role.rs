//! Realm and client roles, and the macros that assert them inside a handler.
//!
//! ISM does not check roles yet — every handler is reachable by any authenticated user — but the
//! `Role` trait is what the `<R>` generic on `KeycloakToken` and `KeycloakAuthLayer` binds, and
//! `String` is the default role type. See `docs/auth.md` for using a custom enum instead.

use std::fmt::{Debug, Display};

use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

/// Describes any type that can act as a role.
pub trait Role: Debug + Display + Clone + PartialEq + Eq + Send + Sync + From<String> {}

/// Roles are read from JSON and are therefore always present as `String`s.
/// Using `String` as the `Role` should be the default when not providing a custom `Role` type.
impl Role for String {}

/// A realm or client role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum KeycloakRole<R: Role> {
    /// A realm role
    Realm {
        /// Name of the role
        role: R,
    },
    /// A client role
    Client {
        /// Client ID
        client: String,
        /// Name of the role
        role: R,
    },
}

impl<R: Role> KeycloakRole<R> {
    /// The role name, whether it came from the realm or from a client.
    pub fn role(&self) -> &R {
        match self {
            KeycloakRole::Realm { role } => role,
            KeycloakRole::Client { client: _, role } => role,
        }
    }
}

/// How many roles a claim section carries, so `ExtractRoles` can size its target vec once.
pub trait NumRoles {
    fn num_roles(&self) -> usize;
}

impl<T: NumRoles> NumRoles for Option<T> {
    fn num_roles(&self) -> usize {
        self.as_ref().map(|it| it.num_roles()).unwrap_or(0)
    }
}

/// Flattens a claim section (`realm_access`, `resource_access`) into a single role list.
pub trait ExtractRoles<R: Role> {
    fn extract_roles(self, target: &mut Vec<KeycloakRole<R>>);
}

/// If type `T` implements `ExtractRoles`, `ExtractRoles` should also be implemented for `Option<T>`,
/// as this impl can just extract the roles if there is a value present.
impl<R: Role, T: ExtractRoles<R>> ExtractRoles<R> for Option<T> {
    fn extract_roles(self, target: &mut Vec<KeycloakRole<R>>) {
        if let Some(inner) = self {
            inner.extract_roles(target)
        }
    }
}

/// If two type `A` and `B` implement `ExtractRoles` (with the impl above this might as well be an `Option<T>`),
/// `ExtractRoles` should be implemented for the tuple (A, B). Given an empty Vec, this only allocates once to fill the vec with all elements.
impl<R: Role, A, B> ExtractRoles<R> for (A, B)
where
    A: NumRoles + ExtractRoles<R>,
    B: NumRoles + ExtractRoles<R>,
{
    fn extract_roles(self, target: &mut Vec<KeycloakRole<R>>) {
        target.reserve(self.0.num_roles() + self.1.num_roles());
        self.0.extract_roles(target);
        self.1.extract_roles(target);
    }
}

/// Asserts the presence (or absence) of roles on something that carries them.
pub trait ExpectRoles<R: Role> {
    type Rejection: IntoResponse;

    fn expect_roles<I: Into<R> + Clone>(&self, roles: &[I]) -> Result<(), Self::Rejection>;
    fn not_expect_roles<I: Into<R> + Clone>(&self, roles: &[I]) -> Result<(), Self::Rejection>;
}

// The four macros below are `#[macro_export]`ed, so they land at the crate root
// (`crate::expect_role!`) regardless of this module being private. They must therefore reach
// `ExpectRoles` through the `auth` facade rather than through `auth::role`.

/// Returns from the handler with an error response unless the token carries all `$roles`.
#[macro_export]
macro_rules! expect_roles {
    ($token: expr, $roles: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::expect_roles($token, $roles) {
            return axum::response::IntoResponse::into_response(err);
        }
    };
}

/// Returns from the handler with an error response unless the token carries `$role`.
#[macro_export]
macro_rules! expect_role {
    ($token: expr, $role: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::expect_roles($token, &[$role]) {
            return axum::response::IntoResponse::into_response(err);
        }
    };
}

/// Returns from the handler with an error response if the token carries any of `$roles`.
#[macro_export]
macro_rules! not_expect_roles {
    ($token: expr, $roles: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::not_expect_roles($token, $roles) {
            return axum::response::IntoResponse::into_response(err);
        }
    };
}

/// Returns from the handler with an error response if the token carries `$role`.
#[macro_export]
macro_rules! not_expect_role {
    ($token: expr, $role: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::not_expect_roles($token, &[$role]) {
            return axum::response::IntoResponse::into_response(err);
        }
    };
}
