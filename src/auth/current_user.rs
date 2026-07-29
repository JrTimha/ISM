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
/// A handler behind the `KeycloakAuthLayer` is only reached once that token exists, so the
/// `MissingToken` rejection below is unreachable there. It is what a handler mounted *outside* the
/// layer gets — a routing mistake, answered as 401 rather than a panic.
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
    use crate::core::errors::AppResponse;
    use crate::{expect_role, not_expect_role};
    use axum::Json;

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

    /// The shape every handler under `/api/v1` actually has. Kept alongside the `-> Response` one
    /// above because the macros have to compile in both, and a single `return` expression can only
    /// produce one type — see `FromRoleRejection`.
    fn admin_only_api(user: &CurrentUser) -> AppResponse<Json<&'static str>> {
        expect_role!(user, AppRole::Admin);
        Ok(Json("ok"))
    }

    /// The denying direction, in the same shape. `not_expect_roles` consults every role source
    /// rather than realm roles only, so it needs its own coverage.
    fn no_guides_api(user: &CurrentUser) -> AppResponse<Json<&'static str>> {
        not_expect_role!(user, AppRole::LocalGuide);
        Ok(Json("ok"))
    }

    /// The regression guard for the macros' return type. They used to expand to
    /// `IntoResponse::into_response(err)`, which is a `Response` and therefore a type error in a
    /// handler returning `AppResponse` — i.e. in every handler ISM has. It went unnoticed because
    /// the only coverage was the `-> Response` case above, where it happened to compile.
    ///
    /// This test failing to *compile* is the point; the assertions merely pin the outcome.
    #[test]
    fn expect_role_macro_works_in_an_app_response_handler() {
        assert!(admin_only_api(&token_with(vec![AppRole::Admin])).is_ok());

        let rejection = admin_only_api(&token_with(vec![AppRole::User]))
            .expect_err("a caller without the role must be rejected");

        // Identical to what the `-> Response` path produces: the handler shape must not be
        // visible to the caller.
        assert_eq!(
            rejection.into_response().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn not_expect_role_macro_works_in_an_app_response_handler() {
        assert!(no_guides_api(&token_with(vec![AppRole::User])).is_ok());

        let rejection = no_guides_api(&token_with(vec![AppRole::LocalGuide]))
            .expect_err("a caller holding the denied role must be rejected");
        assert_eq!(
            rejection.into_response().status(),
            StatusCode::FORBIDDEN
        );
    }
}
