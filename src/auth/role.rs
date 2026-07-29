//! Realm and client roles, and the macros that assert them inside a handler.
//!
//! ISM does not check roles yet — every handler is reachable by any authenticated user — but the
//! `Role` trait is what the `<R>` generic on `KeycloakToken` and `KeycloakAuthLayer` binds, and
//! `String` is the default role type. See `docs/auth.md` for using a custom enum instead.

use std::fmt::{Debug, Display};

use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::auth::error::AuthError;
use crate::core::errors::AppError;

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
    ///
    /// Deliberately *not* what an access check should compare against — see `realm_role`. Keycloak
    /// puts the roles of every client the user holds roles on into `resource_access`, and
    /// `ExtractRoles` flattens those into the same list as the realm roles. Matching on the bare
    /// name therefore treats a client role named `ADMIN`, defined by some unrelated service in the
    /// realm, as if the realm itself had granted it.
    pub fn role(&self) -> &R {
        match self {
            KeycloakRole::Realm { role } => role,
            KeycloakRole::Client { client: _, role } => role,
        }
    }

    /// The role name if the *realm* granted it, `None` for a client role.
    ///
    /// This is what `ExpectRoles::expect_roles` matches on, so that granting a client role can
    /// never widen a caller's access here.
    pub fn realm_role(&self) -> Option<&R> {
        match self {
            KeycloakRole::Realm { role } => Some(role),
            KeycloakRole::Client { .. } => None,
        }
    }

    /// The role name if `client` granted it, `None` otherwise.
    pub fn client_role(&self, client: &str) -> Option<&R> {
        match self {
            KeycloakRole::Client {
                client: owner,
                role,
            } if owner == client => Some(role),
            _ => None,
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
///
/// The two directions deliberately consult different role sources, so that both fail closed:
///
/// - `expect_roles` grants access and therefore only accepts **realm** roles. A client role of the
///   same name must never be enough, or any service in the realm could widen access to ISM by
///   naming one of its own roles `ADMIN`.
/// - `not_expect_roles` denies access and therefore considers **every** role source. A role that
///   should keep a caller out must do so no matter who granted it.
pub trait ExpectRoles<R: Role> {
    type Rejection: IntoResponse;

    fn expect_roles<I: Into<R> + Clone>(&self, roles: &[I]) -> Result<(), Self::Rejection>;
    fn not_expect_roles<I: Into<R> + Clone>(&self, roles: &[I]) -> Result<(), Self::Rejection>;
}

/// What a failed role assertion returns from the handler that asserted it.
///
/// The `expect_role!` family expands to a bare `return`, so the value it produces has to *be* the
/// handler's own return type. ISM has two handler shapes — `-> Response` for the raw ones and
/// `-> AppResponse<Json<T>>` for everything under `/api/v1` — and this trait is what lets a single
/// macro serve both: the impl is selected from the return type at the call site, so no handler ever
/// has to say which shape it is.
///
/// Hard-coding `IntoResponse::into_response` here instead, as the macros used to, only ever
/// produced a `Response`. That made every `expect_role!` in an `AppResponse` handler — which is
/// every handler ISM actually has — a type error, and the only test covering the macros happened to
/// use the one shape where it compiled.
pub trait FromRoleRejection {
    fn from_role_rejection(rejection: AuthError) -> Self;
}

impl FromRoleRejection for Response {
    fn from_role_rejection(rejection: AuthError) -> Self {
        rejection.into_response()
    }
}

/// Covers `AppResponse<T>`, which is this alias. The conversion is lossless: both role rejections
/// classify to the same 403 that `AppError::Forbidden` renders — see `AuthError::into_app_error`.
impl<T> FromRoleRejection for Result<T, AppError> {
    fn from_role_rejection(rejection: AuthError) -> Self {
        Err(rejection.into_app_error())
    }
}

// The four macros below are `#[macro_export]`ed, so they land at the crate root
// (`crate::expect_role!`) regardless of this module being private. They must therefore reach
// `ExpectRoles` and `FromRoleRejection` through the `auth` facade rather than through `auth::role`.

/// Returns from the handler with an error response unless the token carries all `$roles`.
#[macro_export]
macro_rules! expect_roles {
    ($token: expr, $roles: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::expect_roles($token, $roles) {
            return $crate::auth::FromRoleRejection::from_role_rejection(err);
        }
    };
}

/// Returns from the handler with an error response unless the token carries `$role`.
#[macro_export]
macro_rules! expect_role {
    ($token: expr, $role: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::expect_roles($token, &[$role]) {
            return $crate::auth::FromRoleRejection::from_role_rejection(err);
        }
    };
}

/// Returns from the handler with an error response if the token carries any of `$roles`.
#[macro_export]
macro_rules! not_expect_roles {
    ($token: expr, $roles: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::not_expect_roles($token, $roles) {
            return $crate::auth::FromRoleRejection::from_role_rejection(err);
        }
    };
}

/// Returns from the handler with an error response if the token carries `$role`.
#[macro_export]
macro_rules! not_expect_role {
    ($token: expr, $role: expr) => {
        if let Err(err) = $crate::auth::ExpectRoles::not_expect_roles($token, &[$role]) {
            return $crate::auth::FromRoleRejection::from_role_rejection(err);
        }
    };
}
