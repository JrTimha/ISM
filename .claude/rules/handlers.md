---
paths:
  - src/**/handler.rs
---

# Handler Rules

## Strict Separation

- **No business logic in handlers.** Handlers only extract inputs, call the service, and return the result.
- Business logic, validation, and error handling belong in the service layer.

## Auth Extraction

Every protected handler extracts the caller's identity via:

```rust
Extension(claims): Extension<KeycloakClaims>
```

The caller's UUID is available as `claims.sub`.

## Return Type

All handlers return `Result<Json<T>, HttpError>`. On success: `Ok(Json(...))`. On failure: `Err(HttpError)`.

`HttpError` serializes to:
```json
{ "status": 404, "errorCode": "NOT_FOUND", "message": "...", "timestamp": "...", "path": "/api/..." }
```

The `path` field is injected automatically by the `inject_request_path` middleware — do not set it manually.

## No unwrap()

Never use `unwrap()` or `expect()` in handlers. Propagate errors with `?` and convert via `HttpError`.