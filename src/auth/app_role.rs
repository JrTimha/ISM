//! The realm roles ISM knows about.
//!
//! This is the concrete `Role` implementation the whole application is generic over — see
//! `CurrentUser` in `current_user.rs`, which pins `KeycloakToken` to it.

use std::fmt::{Display, Formatter};

use crate::auth::role::Role;

/// A Keycloak realm role, resolved to the set ISM cares about.
///
/// Keycloak hands every user a number of roles nobody here asked for — `offline_access`,
/// `uma_authorization`, `default-roles-<realm>`, plus the client roles of the `account` client.
/// Those land in `Unknown` and simply never match a check, which is why this enum has a catch-all
/// instead of failing to parse: an unrecognised role is normal, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRole {
    Admin,
    User,
    LocalGuide,
    /// Any role the realm hands out that ISM has no rule for. Carries the original name.
    Unknown(String),
}

impl AppRole {
    /// The role name exactly as the realm spells it.
    ///
    /// `Display` and `From<String>` both go through this, so `AppRole::from(role.to_string())`
    /// round-trips for every variant — including `Unknown`.
    pub fn as_str(&self) -> &str {
        match self {
            AppRole::Admin => "ADMIN",
            AppRole::User => "USER",
            AppRole::LocalGuide => "LOCAL_GUIDE",
            AppRole::Unknown(name) => name,
        }
    }
}

impl Role for AppRole {}

impl Display for AppRole {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<String> for AppRole {
    fn from(value: String) -> Self {
        match value.as_str() {
            "ADMIN" => AppRole::Admin,
            "USER" => AppRole::User,
            "LOCAL_GUIDE" => AppRole::LocalGuide,
            _ => AppRole::Unknown(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppRole;

    #[test]
    fn maps_the_realm_role_names() {
        assert_eq!(AppRole::from("ADMIN".to_owned()), AppRole::Admin);
        assert_eq!(AppRole::from("USER".to_owned()), AppRole::User);
        assert_eq!(AppRole::from("LOCAL_GUIDE".to_owned()), AppRole::LocalGuide);
    }

    #[test]
    fn keeps_unknown_roles_verbatim() {
        // Keycloak assigns these to every account; they must survive as-is rather than being
        // dropped, so a log line naming the role is still readable.
        for name in [
            "offline_access",
            "uma_authorization",
            "default-roles-meventure",
        ] {
            assert_eq!(
                AppRole::from(name.to_owned()),
                AppRole::Unknown(name.to_owned())
            );
        }
    }

    #[test]
    fn role_names_are_case_sensitive() {
        // Keycloak role names are case-sensitive, so "admin" is genuinely a different role than
        // "ADMIN" and must not silently be treated as one.
        assert_eq!(
            AppRole::from("admin".to_owned()),
            AppRole::Unknown("admin".to_owned())
        );
    }

    #[test]
    fn display_round_trips_through_from_string() {
        for role in [
            AppRole::Admin,
            AppRole::User,
            AppRole::LocalGuide,
            AppRole::Unknown("something-else".to_owned()),
        ] {
            assert_eq!(AppRole::from(role.to_string()), role);
        }
    }
}
