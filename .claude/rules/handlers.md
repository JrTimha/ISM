---
paths:
  - src/**/handler.rs
---

# Handler Rules

## Strict Separation

- **No business logic in handlers.** Handlers only extract inputs, call the service, and return the result.
- Business logic, validation, and error handling belong in the service layer.

**Where the line falls:** the handler validates *syntax* — the `validator` bounds on its request
type, `decode_cursor`, the `PageSize` clamp, pulling a field out of a multipart body. Anything that
requires reading the database, **including authorization**, is the service's. "Is this user in this
room?" is not a handler question; `RoomService` answers it for every caller, so a new endpoint
cannot forget to ask.

## Request Extraction

**Take `ValidatedJson<T>` / `ValidatedQuery<T>`, never bare `Json<T>` / `Query<T>`.**

```rust
pub async fn handle_create_room(
    State(rooms): State<RoomService>,
    user: CurrentUser,
    ValidatedJson(payload): ValidatedJson<NewRoomRequest>,
) -> Result<Json<RoomResponse>, AppError> {
    let room = rooms.create_room(user.subject, payload).await?;
    Ok(Json(room))
}
```

Both extractors (`core/extract.rs`) are bound on `ApiRequest`, whose `Validate` supertrait makes a
request type without rules impossible to register, and both run `validate()` before the handler body
starts. A handler that compiles has validated its input.

This replaced a hand-written `payload.validate().map_err(AppError::from)?` that existed at two call
sites in the whole project — one of which checked a single nested field and let an unbounded room
name and an uncapped invite list through. Deserialization failures and rule violations both become
`AppError::Validation`, so the client sees one 400 shape either way.

`limit` is *not* validated: a value above `MAX_PAGE_SIZE` is not a client error, it is a request for
more than the server serves. Declare it as `PageSize`, which clamps while deserializing — see
`.claude/rules/pagination.md`.

See `.claude/rules/model.md` for the request/response/row/JSONB taxonomy these types belong to.

## Dependency Injection

Take the one service you use, never the whole state:

```rust
pub async fn handle_leave_room(
    user: CurrentUser,
    State(rooms): State<RoomService>,
    Path(room_id): Path<Uuid>,
) -> AppResponse<()> {
    rooms.leave_room(user.subject, room_id).await?;
    Ok(())
}
```

axum resolves `State<RoomService>` through the `FromRef` impls in `core/app_state.rs`. Never write
`State<Arc<AppState>>` in a handler — it compiles, but it puts every service back within reach and
makes the signature say nothing about what the handler actually touches. A handler needing two
services asks for two; needing three usually means the use case belongs in a service.

## Auth Extraction

Every protected handler takes the caller as:

```rust
use crate::auth::CurrentUser;

user: CurrentUser          // = KeycloakToken<AppRole>
```

`CurrentUser` is the full validated token. The caller's id is `user.subject` (`Uuid`, `Copy`);
also available are `user.roles`, `user.extra.profile.preferred_username`, `user.extra.email` and
the standard JWT claims.

Name the binding `user` and use `user.subject` inline. Only bind a local `let user_id =
user.subject;` when the id outlives the handler body — a `move` closure or a spawned task — so the
capture is a `Copy` id rather than the whole token (see `messaging/handler.rs`).

Path parameters are named after what they refer to (`target_id`, `friend_id`, `sender_id`,
`invited_user_id`), never `user_id`.

**Roles** are not enforced anywhere yet. To require one in a handler:

```rust
expect_role!(&user, AppRole::Admin);   // returns 403 early if absent
```

See `../../.docs/auth.md`.

## Return Type

All handlers return `AppResponse<Json<T>>` (= `Result<Json<T>, AppError>`, from
`crate::core::errors`). On success: `Ok(Json(...))`. On failure: `Err(AppError)`.

`AppError` serializes to:
```json
{ "timestamp": "...", "status": 404, "error": "Not Found", "message": "...", "path": "/api/v1/...", "errorCode": "CONTENT_NOT_FOUND" }
```

The `path` field is injected automatically by the `inject_request_path` middleware — do not set it manually.

Pick the variant by audience: `Validation` / `NotFound` / `Forbidden` pass their message through to
the caller; `Database` / `Cache` / `Serialization` / `S3` / `Processing` are logged in full and
answered with a generic message.

## No unwrap()

Never use `unwrap()` or `expect()` in handlers. Propagate errors with `?` and convert via `AppError`.