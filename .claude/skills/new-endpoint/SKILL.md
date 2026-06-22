---
name: new-endpoint
description: Scaffold a new API endpoint in the ISM project following the established layered architecture (repository → service → handler → route). Use when adding new HTTP endpoints.
disable-model-invocation: true
argument-hint: <module> <HTTP-method> <path>
allowed-tools: Read Grep Edit Bash(cargo check) Bash(cargo sqlx prepare)
---

Scaffold a new API endpoint in the ISM project following the established layered architecture.

Input: $ARGUMENTS
Format: `<module> <HTTP-method> <path>` — e.g. `rooms POST /api/rooms/{id}/pin`

## Your Task

First read the existing files of the given module to match the style:
- `src/<module>/handler.rs`
- `src/<module>/routes.rs`
- the corresponding service and repository file

Then implement in this order:

### 1. Repository (`src/<module>/repository/`)
- New function with the correct SQLx executor signature (read `docs/sqlx-executor-pattern.md`)
- Query using `sqlx::query!` / `sqlx::query_as!` macro
- Afterwards: run `cargo sqlx prepare` and remind to commit `.sqlx/`

### 2. Service (`src/<module>/*_service.rs`)
- Business logic, validation, error handling via `HttpError`
- Calls the repository function

### 3. Handler (`src/<module>/handler.rs`)
- Extract `Extension(claims): Extension<KeycloakClaims>` for auth
- Call service, return `Ok(Json(...))` or `Err(HttpError)`
- No business logic in the handler

### 4. Route (`src/<module>/routes.rs`)
- Register the route in the router
- Correct HTTP method and path

### 5. Final check
- Run `cargo check`
- Flag any open `unwrap()` calls or missing error handling