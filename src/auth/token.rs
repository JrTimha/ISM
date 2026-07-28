//! The claim types a validated JWT is parsed into.
//!
//! `StandardClaims` mirrors the JSON Keycloak emits; `KeycloakToken` is the typed form handed to
//! handlers through the request extensions. The validation that produces them lives in
//! `decode.rs`.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_with::{OneOrMany, serde_as};
use uuid::Uuid;

use crate::auth::error::AuthError;
use crate::auth::role::{ExpectRoles, ExtractRoles, KeycloakRole, NumRoles, Role};

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardClaims<Extra> {
    /// Expiration time (unix timestamp).
    pub exp: i64,
    /// Issued at time (unix timestamp).
    pub iat: i64,
    /// JWT ID (unique identifier for this token).
    pub jti: String,
    /// Issuer (who created and signed this token).
    pub iss: String,
    /// Audience (who or what the token is intended for).
    #[serde_as(deserialize_as = "OneOrMany<_>")]
    #[serde(default)]
    pub aud: Vec<String>,
    /// Subject (whom the token refers to).
    pub sub: String,
    /// Type of token.
    pub typ: String,
    /// Authorized party (the party to which this token was issued).
    pub azp: String,

    /// Keycloak: Optional realm roles from Keycloak.
    pub realm_access: Option<RealmAccess>,
    /// Keycloak: Optional client roles from Keycloak.
    pub resource_access: Option<ResourceAccess>,

    #[serde(flatten)]
    pub extra: Extra,
}

/// Access details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Access {
    /// A list of role names.
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmAccess(pub Access);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceAccess(pub HashMap<String, Access>);

impl NumRoles for RealmAccess {
    fn num_roles(&self) -> usize {
        self.0.roles.len()
    }
}

impl NumRoles for ResourceAccess {
    fn num_roles(&self) -> usize {
        self.0.values().map(|access| access.roles.len()).sum()
    }
}

impl<R: Role> ExtractRoles<R> for RealmAccess {
    fn extract_roles(self, target: &mut Vec<KeycloakRole<R>>) {
        for role in self.0.roles {
            target.push(KeycloakRole::Realm { role: role.into() });
        }
    }
}

impl<R: Role> ExtractRoles<R> for ResourceAccess {
    fn extract_roles(self, target: &mut Vec<KeycloakRole<R>>) {
        for (res_name, access) in &self.0 {
            for role in &access.roles {
                target.push(KeycloakRole::Client {
                    client: res_name.to_owned(),
                    role: role.to_owned().into(),
                });
            }
        }
    }
}

/// Token data parsed from the request and added as an `axum::Extension` through our middleware.
///
/// This only exists if the `KeycloakAuthLayer` is configured to use `PassthroughMode::Block`.
///
/// If you want to manually check whether a request was authenticated, configure
/// `PassthroughMode::Pass` (potentially on a separate `axum::Router`) and inject
/// `KeycloakAuthStatus` instead of `KeycloakToken`!
///
/// Handlers do not name this type directly — they take `CurrentUser`, which pins `R` to
/// `AppRole`:
///
/// ```ignore
/// use crate::auth::CurrentUser;
///
/// pub async fn who_am_i(user: CurrentUser) -> Response {
///     let name = &user.extra.profile.preferred_username;
///     // ...
/// }
/// ```
#[derive(Debug, PartialEq, Clone)]
pub struct KeycloakToken<R, Extra = ProfileAndEmail>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    /// Expiration time (UTC).
    pub expires_at: time::OffsetDateTime,
    /// Issued at time (UTC).
    pub issued_at: time::OffsetDateTime,
    /// JWT ID (unique identifier for this token).
    pub jwt_id: String,
    /// Issuer (who created and signed this token).
    pub issuer: String,
    /// Audience (who or what the token is intended for).
    pub audience: Vec<String>,
    /// Subject (whom the token refers to). This is the UUID which uniquely identifies this user inside Keycloak.
    pub subject: Uuid,
    /// Authorized party (the party to which this token was issued).
    pub authorized_party: String,

    // Keycloak: Roles of the user.
    pub roles: Vec<KeycloakRole<R>>,

    pub extra: Extra,
}

impl<R, Extra> KeycloakToken<R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    pub fn parse(raw: StandardClaims<Extra>) -> Result<Self, AuthError> {
        Ok(Self {
            expires_at: time::OffsetDateTime::from_unix_timestamp(raw.exp).map_err(|err| {
                AuthError::InvalidToken {
                    reason: format!(
                        "Could not parse 'exp' (expires_at) field as unix timestamp: {err}"
                    ),
                }
            })?,
            issued_at: time::OffsetDateTime::from_unix_timestamp(raw.iat).map_err(|err| {
                AuthError::InvalidToken {
                    reason: format!(
                        "Could not parse 'iat' (issued_at) field as unix timestamp: {err}"
                    ),
                }
            })?,
            jwt_id: raw.jti,
            issuer: raw.iss,
            audience: raw.aud,
            subject: Uuid::try_parse(&raw.sub).map_err(|err| AuthError::InvalidToken {
                reason: format!("Could not parse 'sub' (subject) field as uuid: {err}"),
            })?,
            authorized_party: raw.azp,
            roles: {
                let mut roles = Vec::new();
                (raw.realm_access, raw.resource_access).extract_roles(&mut roles);
                roles
            },
            extra: raw.extra,
        })
    }

    pub fn is_expired(&self) -> bool {
        time::OffsetDateTime::now_utc() > self.expires_at
    }

    pub fn assert_not_expired(&self) -> Result<(), AuthError> {
        match self.is_expired() {
            true => Err(AuthError::TokenExpired),
            false => Ok(()),
        }
    }
}

impl<R, Extra> ExpectRoles<R> for KeycloakToken<R, Extra>
where
    R: Role,
    Extra: DeserializeOwned + Clone,
{
    type Rejection = AuthError;

    fn expect_roles<I: Into<R> + Clone>(&self, roles: &[I]) -> Result<(), Self::Rejection> {
        for expected in roles {
            let expected: R = expected.clone().into();
            if !self.roles.iter().any(|role| role.role() == &expected) {
                return Err(AuthError::MissingExpectedRole {
                    role: expected.to_string(),
                });
            }
        }
        Ok(())
    }

    fn not_expect_roles<I: Into<R> + Clone>(&self, roles: &[I]) -> Result<(), Self::Rejection> {
        for expected in roles {
            let expected: R = expected.clone().into();
            if let Some(_role) = self.roles.iter().find(|role| role.role() == &expected) {
                return Err(AuthError::UnexpectedRole);
            }
        }
        Ok(())
    }
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct Profile {
    /// Keycloak: Username of the user.
    pub preferred_username: String,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct Email {
    /// Keycloak: Email address of the user.
    pub email: Option<String>,
    /// Keycloak: Whether the users email is verified.
    pub email_verified: bool,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct ProfileAndEmail {
    #[serde(flatten)]
    pub profile: Profile,
    #[serde(flatten)]
    pub email: Email,
}
