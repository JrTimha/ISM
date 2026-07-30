//! Keycloak JWT authentication for the protected axum routes.
//!
//! The layer is built once in `middleware::auth` and wrapped around the protected router. A
//! request only reaches a handler once its bearer token has been validated against the realm's
//! published signing keys; anything else is answered with an error and never gets that far.
//!
//! Request path:
//!
//! | File | Responsibility |
//! |---|---|
//! | `layer.rs` | tower `Layer`; the per-layer config — required roles and where to find the token |
//! | `service.rs` | per-request tower `Service`; inserts the validated token or answers the error |
//! | `extract.rs` | pulls the raw JWT out of the request (header, query param) |
//! | `policy.rs` | `ValidationPolicy` — the rules a token is held to, fixed at startup |
//! | `decode.rs` | runs those rules: signature and claim validation |
//! | `token.rs` | the claim types validation produces, `KeycloakToken` among them |
//! | `instance.rs` | one realm: OIDC discovery, the `ValidationPolicy`, the JWKS `watch` every request reads, on-demand refresh on key rotation |
//! | `oidc.rs` / `oidc_discovery.rs` | discovery document DTOs / the HTTP calls fetching them |
//! | `role.rs` | the generic `Role` trait and the `expect_role!` family of macros |
//! | `app_role.rs` | `AppRole` — the realm's roles, the concrete `Role` this app uses |
//! | `current_user.rs` | `CurrentUser` = `KeycloakToken<AppRole>`, plus its extractor |
//!
//! Handlers behind the layer take `CurrentUser`, which is the whole validated token: caller id,
//! roles, profile and email claims. The `<R>` generic is pinned to `AppRole` there, so no handler
//! and no route ever spells it out.
//!
//! Full usage guide, including custom role types and custom token extractors: `docs/auth.md`.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]

mod app_role;
mod current_user;
mod decode;
mod error;
mod extract;
mod instance;
mod layer;
mod oidc;
mod policy;
mod oidc_discovery;
mod role;
#[cfg(test)]
mod security_tests;
mod service;
mod token;

pub use app_role::AppRole;
pub use current_user::CurrentUser;
pub use error::AuthError;
// Several `AuthError` variants keep their `Display` free of the source, so `{err}` alone drops the
// actual cause. Startup needs the full chain to make a failed discovery diagnosable from a crash log.
pub(crate) use error::error_chain;
pub use instance::{KeycloakAuthInstance, KeycloakConfig};
pub use layer::KeycloakAuthLayer;
pub use policy::ValidationPolicy;
pub use token::KeycloakToken;
// Re-exported for the `expect_role!` family of macros, which expand to `$crate::auth::ExpectRoles`
// and `$crate::auth::FromRoleRejection` at the call site and therefore cannot reach through the
// private `role` module.
pub use role::{ExpectRoles, FromRoleRejection, KeycloakRole, Role};
