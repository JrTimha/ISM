//! Extractors that make request validation structural instead of remembered.
//!
//! axum's `Json<T>` and `Query<T>` deserialize and stop there. Running `validator` was left to the
//! handler, which meant it happened where someone remembered: `validate()` was called at two sites
//! in the entire codebase, and one of them checked a single nested field of its payload while the
//! rest — an unbounded room name, an uncapped invite list — went straight to the service.
//!
//! [`ValidatedJson`] and [`ValidatedQuery`] close that by construction. Both are bound on
//! [`ApiRequest`], whose `Validate` supertrait means a type without validation rules cannot be
//! extracted at all, and both run `validate()` before the handler body starts. A handler that
//! compiles has validated its input.
//!
//! ```ignore
//! pub async fn handle_create_room(
//!     user: CurrentUser,
//!     State(rooms): State<RoomService>,
//!     ValidatedJson(request): ValidatedJson<NewRoomRequest>,
//! ) -> AppResponse<Json<RoomResponse>> {
//!     Ok(Json(rooms.create_room(user.subject, request).await?))
//! }
//! ```
//!
//! Both reject with [`AppError::Validation`], so a malformed body and a body that breaks a rule
//! produce the same 400 envelope with `errorCode: "VALIDATION_ERROR"`; see `.claude/rules/handlers.md`.

use crate::core::errors::AppError;
use crate::core::model::ApiRequest;
use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;

/// A JSON request body that has been deserialized **and** validated.
///
/// Use for every `POST`/`PUT`/`PATCH` body. Destructure it in the handler signature —
/// `ValidatedJson(request): ValidatedJson<NewRoomRequest>` — so the body sees the inner type.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatedJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: ApiRequest,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await.map_err(json_rejection)?;
        value.validate()?;
        Ok(ValidatedJson(value))
    }
}

/// A query string that has been deserialized **and** validated.
///
/// Use for every list endpoint's filter/cursor/limit struct. Note that `limit` is clamped rather
/// than validated — declare it as [`PageSize`](crate::core::cursor::PageSize), which caps the value
/// while deserializing. A limit above `MAX_PAGE_SIZE` is not a client error, it is a request for
/// more than the server gives.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatedQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidatedQuery<T>
where
    T: ApiRequest,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state).await.map_err(query_rejection)?;
        value.validate()?;
        Ok(ValidatedQuery(value))
    }
}

/// Turns axum's deserialization rejection into the project's error envelope.
///
/// The rejection's own message is passed through: it names the offending field and expected type,
/// which is exactly the actionable feedback [`AppError::Validation`] is for, and it describes the
/// caller's payload rather than anything about the server.
fn json_rejection(rejection: JsonRejection) -> AppError {
    AppError::Validation(rejection.body_text())
}

fn query_rejection(rejection: QueryRejection) -> AppError {
    AppError::Validation(rejection.body_text())
}
