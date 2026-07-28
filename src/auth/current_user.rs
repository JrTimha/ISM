//! The handler-facing extractor for the authenticated caller.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::auth::app_role::AppRole;
use crate::auth::error::AuthError;
use crate::auth::token::KeycloakToken;

/// The authenticated caller — the full validated token, not just an id.
///
/// This is the concrete instantiation of `KeycloakToken` the whole application uses, which is why
/// no handler ever writes the `<R>` generic. Available on it:
///
/// - `subject` — the Keycloak user UUID, what almost every handler passes on to a service
/// - `roles` — `Vec<KeycloakRole<AppRole>>`, realm and client roles
/// - `extra.profile` — `preferred_username`
/// - `extra.email` — `email`, `email_verified`
/// - `expires_at`, `issued_at`, `issuer`, `audience`, `authorized_party`, `jwt_id`
///
/// ```ignore
/// pub async fn handle_get_friends(
///     State(state): State<Arc<AppState>>,
///     user: CurrentUser,
/// ) -> AppResponse<Json<CursorResults<User>>> {
///     expect_role!(&user, AppRole::Admin);   // optional, see docs/auth.md
///     let results = UserService::get_friends(state, &user.subject, /* … */).await?;
///     Ok(Json(results))
/// }
/// ```
pub type CurrentUser = KeycloakToken<AppRole>;

/// Hands the handler the token the middleware already validated and stashed.
///
/// The `KeycloakAuthLayer` runs in `PassthroughMode::Block`, so a handler behind it is only
/// reached once that token exists. Under `PassthroughMode::Pass` there is no token extension to
/// read and this rejects with 401 — extract `KeycloakAuthStatus` there instead.
impl<S> FromRequestParts<S> for KeycloakToken<AppRole>
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Cloned rather than taken out of the extensions: a handler may name `CurrentUser` more
        // than once, and later layers still expect the extension to be there.
        parts
            .extensions
            .get::<KeycloakToken<AppRole>>()
            .cloned()
            .ok_or(AuthError::MissingToken)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use uuid::Uuid;

    use super::CurrentUser;
    use crate::auth::app_role::AppRole;
    use crate::auth::role::KeycloakRole;
    use crate::auth::token::{Email, Profile, ProfileAndEmail};
    use crate::expect_role;

    fn token_with(roles: Vec<AppRole>) -> CurrentUser {
        CurrentUser {
            expires_at: chrono::Utc::now() + chrono::TimeDelta::minutes(5),
            issued_at: chrono::Utc::now(),
            jwt_id: "b7c1e5a2-3f4d-4e5a-9b8c-7d6e5f4a3b2c".to_owned(),
            issuer: "https://keycloak.example/realms/meventure".to_owned(),
            audience: vec!["account".to_owned()],
            subject: Uuid::now_v7(),
            authorized_party: "ism-app".to_owned(),
            roles: roles
                .into_iter()
                .map(|role| KeycloakRole::Realm { role })
                .collect(),
            extra: ProfileAndEmail {
                profile: Profile {
                    preferred_username: "tim".to_owned(),
                },
                email: Email {
                    email: None,
                    email_verified: false,
                },
            },
        }
    }

    /// Shaped like a real handler, so the macro's early `return` is exercised the way it is at an
    /// actual call site — that is the whole point of the test.
    fn admin_only(user: &CurrentUser) -> Response {
        expect_role!(user, AppRole::Admin);
        StatusCode::OK.into_response()
    }

    #[test]
    fn expect_role_macro_resolves_through_the_auth_facade() {
        // The `expect_role!` family expands to `$crate::auth::ExpectRoles`. Nothing in the
        // application calls these macros yet, so without this test a broken path — which is
        // exactly what a private `role` module would cause — would go unnoticed until the first
        // real use.
        assert_eq!(
            admin_only(&token_with(vec![AppRole::Admin])).status(),
            StatusCode::OK
        );
    }

    #[test]
    fn expect_role_macro_rejects_a_caller_without_the_role() {
        let response = admin_only(&token_with(vec![AppRole::User, AppRole::LocalGuide]));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
