# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**ISM (Instant SaaS Messenger)** is a real-time messaging backend written in Rust. It provides private/group chat rooms, a friend system, media uploads, and live notifications via SSE and WebSockets.

**Stack**: Axum 0.8 + Tokio, PostgreSQL (SQLx), ScyllaDB/Cassandra (messages), Redis (optional cache), MinIO/S3, Keycloak OIDC, Kafka (optional push notifications).

## Commands

```bash
# Build & run
cargo build
cargo run

# After modifying any SQL query, regenerate compile-time metadata:
cargo sqlx prepare

# Database migrations
sqlx migrate run
sqlx migrate add <name>

# Tests
cargo test
cargo test <test_name> -- --nocapture

# Lint / format
cargo clippy
cargo fmt

# Full dev stack (PostgreSQL, Cassandra, Keycloak, Redis, MinIO, Kafka)
docker compose up -d
docker build -t ism:latest .
```

Set `DATABASE_URL` in `.env` for the sqlx CLI. The `.sqlx/` directory holds pre-compiled query metadata — always commit it after running `cargo sqlx prepare`.

## Architecture

### Layers

```
Routes (router.rs)
  ↓ Keycloak JWT middleware → injects KeycloakClaims into request extensions
Handlers (rooms/handler.rs, messaging/handler.rs, user_relationship/handler.rs)
  ↓
Services (room_service.rs, message_service.rs, user_service.rs)
  ↓
Repositories (repository/) ─── PostgreSQL (SQLx)
  ↓
message_service ───────────── Cassandra (ScyllaDB)
BroadcastChannel ──────────── SSE / WebSocket senders (in-memory HashMap)
```

### AppState (`core/app_state.rs`)

Singleton initialized at startup, shared via `Arc<AppState>` across all handlers. Holds the PostgreSQL pool (max 20 connections), Cassandra client, S3 client, cache layer, repositories, and the global `BroadcastChannel`.

### Configuration (`core/config.rs`)

Layered TOML loading: `default.config.toml` → `{mode}.config.toml` → environment variables. Mode is set via `ISM_MODE` env var (default: `development`). Sections: `message_db_config`, `user_db_config`, `token_issuer`, `object_db_config`, `kafka_config`. Env var override format: `ISM_USER_DB_CONFIG__DB_HOST=...`.

### Real-time Broadcasting (`broadcast/`)

`BroadcastChannel` is a global singleton (OnceCell) holding a `HashMap<UUID, broadcast::Sender<Notification>>` — one sender per connected user. SSE and WebSocket handlers subscribe by calling `BroadcastChannel::subscribe(user_id)` and loop receiving events. Notifications are always broadcast **after** successful DB writes. `notification.rs` defines the `NotificationEvent` enum (ChatMessage, RoomUpdated, FriendRequest, ReadStatus, etc.).

### Database Pattern

- **PostgreSQL**: Users, chat messages, rooms, participants, friend requests — anything requiring strong consistency. Queries are compile-time type-checked via SQLx macros against `.sqlx/` metadata.
- **Redis**: Optional caching of relationship states, room member lists. Falls back to NoOp if unavailable.

### Authentication

Keycloak middleware validates the JWT on every request (JWKS endpoint cached). Valid tokens inject `KeycloakClaims` into request extensions. Handlers extract the user UUID via `Extension(claims): Extension<KeycloakClaims>`.

### Key Data Model Facts

- `chat_room_participant` tracks state per (room, user): `Joined`, `Invited`, `Left`. Most endpoints filter to `Joined` only. Rows are never deleted — leaving sets state to `Left` to preserve history.
- `last_message_read_at` per (user, room) drives read receipts. Updated via POST `/api/rooms/{id}/mark-read`; broadcast so all user devices stay in sync.
- Timeline pagination is timestamp-based (not offset), which is efficient with indexed `created_at` and handles concurrent inserts correctly.
- User search uses cursor pagination via `raw_name` index.

### Routing

Three route modules merged in `router.rs`:
- `rooms/routes.rs` → `/api/rooms/*`
- `messaging/routes.rs` → `/api/send-msg`, `/api/sse`, `/api/wss`
- `user_relationship/routes.rs` → `/api/users/*`

Middleware stack (applied to protected routes): TraceLayer → CorsLayer → KeycloakAuthLayer → DefaultBodyLimit (5 MB for multipart uploads).

### Error Handling

All handlers return `Result<Json<T>, HttpError>`. `HttpError` serializes to JSON with `{ status, errorCode, message, timestamp, path }`. Use `ErrorCode` enum variants for consistent classification.

## Development Patterns

**New endpoint**: Add handler to the relevant `handler.rs`, call service functions for business logic, return `Ok(Json(...))` or `Err(HttpError::new(...))`, register the route in the module's `routes.rs`.

**New SQL query**: Write the query in the repository function, run `cargo sqlx prepare`, commit `.sqlx/` changes.

**New message type**: Add to `MsgType` and `MessageBody` enums in `messaging/model.rs`, handle in `message_service.rs` validation, serialize to Cassandra as JSON string in `msg_body`.

**Broadcasting after writes**:
```rust
BroadcastChannel::broadcast(&Notification {
    user_id: Some(target_user_id),
    event: NotificationEvent::ChatMessage(message),
    timestamp: Utc::now(),
}).await;
```

## Production Deployment

1. `docker build -t ism:latest .`
2. Mount `production.config.toml` with real credentials; set `ISM_MODE=production`
3. Run `sqlx migrate run` before starting ISM
4. Set `with_db_init = false` in config (prevents Cassandra auto-init)
5. Health check: `GET /health` → 200