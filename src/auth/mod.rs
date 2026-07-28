//! Keycloak JWT authentication for the protected axum routes.
//!
//! The layer is built once in `router::init_auth` and wrapped around the protected router. In
//! `PassthroughMode::Block` — the mode ISM uses — a request only reaches a handler once its
//! bearer token has been validated against the realm's published signing keys.
//!
//! Request path:
//!
//! | File | Responsibility |
//! |---|---|
//! | `layer.rs` | tower `Layer`, holds the config; built once at startup |
//! | `service.rs` | per-request tower `Service`; blocks or passes through |
//! | `extract.rs` | pulls the raw JWT out of the request (header, query param) |
//! | `decode.rs` | signature and claim validation |
//! | `token.rs` | the claim types validation produces, `KeycloakToken` among them |
//! | `instance.rs` | OIDC discovery, cached JWKS, on-demand refresh on key rotation |
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

mod action;
mod app_role;
mod current_user;
mod decode;
mod error;
mod extract;
mod instance;
mod layer;
mod mode;
mod oidc;
mod oidc_discovery;
mod role;
#[cfg(test)]
mod security_tests;
mod service;
mod token;

pub use app_role::AppRole;
pub use current_user::CurrentUser;
pub use decode::ValidationPolicy;
pub use error::AuthError;
pub use instance::{KeycloakAuthInstance, KeycloakConfig};
pub use layer::KeycloakAuthLayer;
pub use mode::{KeycloakAuthStatus, PassthroughMode};
pub use token::KeycloakToken;
// Re-exported for the `expect_role!` family of macros, which expand to `$crate::auth::ExpectRoles`
// at the call site and therefore cannot reach through the private `role` module.
pub use role::{ExpectRoles, KeycloakRole, Role};
