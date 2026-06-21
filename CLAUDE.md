# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Language Policy

All code, comments, documentation, commit messages, variable names, error messages, and API responses must be written in **English**. No German anywhere in the codebase.

## Project Vision

ISM is being built as a highly scalable social backend for real-time messaging — supporting 1-1 and group (1-n) chat, a full user relationship system, and eventually a complete real-time social platform.

**Current phase**: single-server, feature-complete messaging backend.
**Next phase**: horizontal scaling via server cluster / federation.
**Long-term features planned**: image/video uploads, voice messages, polls/votings, reactions, activity feeds.

**Non-negotiable quality bars**:
- Strict cursor-based pagination everywhere — no `page`/`pageSize` parameters anywhere in the API.
- Performance-conscious data access — no N+1 queries, indexed lookups, efficient JSONB usage.
- Correctness over convenience — no `unwrap()` in production paths, no silent fallbacks that hide bugs.

## Project Overview

**ISM (Instant SaaS Messenger)** is a real-time messaging backend written in Rust. It provides 1-1 and group chat rooms, a friend/block/invite relationship system, media uploads, and live notifications via SSE and WebSockets.

**Stack**: Axum 0.8 + Tokio, PostgreSQL with SQLx (all data including messages), Redis (optional notification cache), MinIO/S3 (media), Keycloak OIDC (auth), Kafka (optional push notifications).

> **Note**: ScyllaDB/Cassandra has been fully removed. PostgreSQL is the single source of truth for all data.

## Commands

```bash
# Build & run
cargo build
cargo run

# After modifying any SQL query, regenerate compile-time metadata:
cargo sqlx prepare

# Database migrations
sqlx migrate run
sqlx migrate add <name>      # creates migrations/<timestamp>_<name>.up.sql + .down.sql

# Tests
cargo test
cargo test <test_name> -- --nocapture

# Lint / format
cargo clippy
cargo fmt

# Full dev stack (PostgreSQL, Keycloak, Redis, MinIO, Kafka)
docker compose up -d
docker build -t ism:latest .
```

Set `DATABASE_URL` in `.env` for the sqlx CLI. The `.sqlx/` directory holds pre-compiled query metadata — always commit it after running `cargo sqlx prepare`.

## Architecture

### Layers

```
Routes (router.rs)
  ↓ Keycloak JWT middleware → injects KeycloakClaims into request extensions
Handlers (rooms/handler.rs, messaging/handler.rs, users/handler.rs)
  ↓
Services (room_service.rs, timeline_service.rs, message_service.rs, user_service.rs)
  ↓
Repositories (room_repository.rs, chat_repository.rs, user_repository.rs) ─── PostgreSQL (SQLx)
BroadcastChannel ──────────── SSE / WebSocket senders (in-memory HashMap, OnceCell singleton)
```

### AppState (`core/app_state.rs`)

Singleton initialized at startup, shared as `Arc<AppState>` across all handlers. Fields:

| Field | Type | Purpose |
|---|---|---|
| `env` | `ISMConfig` | Full config snapshot |
| `room_repository` | `RoomRepository` | Rooms, participants, read states |
| `user_repository` | `UserRepository` | Users, relationships |
| `chat_repository` | `ChatRepository` | Chat messages |
| `cache` | `Arc<dyn Cache>` | Redis or `NoOpCache` fallback |
| `s3_bucket` | `ObjectStorage` | MinIO/S3 media uploads |

PostgreSQL pool (max 20 connections) is shared across all three repositories.

### Configuration (`core/config.rs`)

Layered TOML loading: `default.config.toml` → `{mode}.config.toml` → environment variables.
Mode via `ISM_MODE` env var (default: `development`).

Config sections:
- `room_db_config` — PostgreSQL connection (host, port, user, password, db_name)
- `token_issuer` — Keycloak host + realm
- `object_db_config` — MinIO/S3 credentials
- `kafka_config` — Kafka bootstrap, topic, partition, consumer group
- `redis_cache_url` — optional Redis URL (omit to use `NoOpCache`)
- `use_kafka` — bool, enables Kafka push notification producer
- `cors_origin` — allowed CORS origin

Env var override format: `ISM_ROOM_DB_CONFIG__DB_HOST=...`

### Real-time Broadcasting (`broadcast/`)

`BroadcastChannel` is a global singleton (`OnceCell<Arc<BroadcastChannel>>`). It holds a `RwLock<HashMap<Uuid, Sender<Notification>>>` — one Tokio broadcast channel per connected user.

**API**:
```rust
BroadcastChannel::get().send_event(notification, &user_id).await;
BroadcastChannel::get().send_event_to_all(user_ids, notification).await;
BroadcastChannel::get().subscribe_to_user_events(user_id).await; // → Receiver
BroadcastChannel::get().unsubscribe(user_id).await;
```

**Rules**:
- Always broadcast **after** a successful DB write, never before.
- Build notifications with `Notification::new(body)`; `seq` is assigned per-user during delivery, not at construction.
- `send_event` / `send_event_to_all` assign a monotonic **per-user** `seq` (Redis `INCR`), cache durable events in a per-user Redis Stream (`user_notifications:{id}`, entry ID `<seq>-0`, length-capped via `XADD ... MAXLEN ~ N` — no background cleanup), and fall back to Kafka push notifications for offline users.
- **Ephemeral** events (`NotificationEvent::is_ephemeral()`) get no `seq` and are never cached — live-only (e.g. `Resync`, future typing indicators).
- Push notifications are only sent for: `ChatMessage`, `FriendRequestReceived`, `NewRoom`.
- Wire envelope: `{ v, seq, type, createdAt, ...payload }`. Clients reconnect with `?last_seq=<n>` on `/api/sse` and `/api/wss`; the server replays missing durable events or emits a `Resync` when the gap was trimmed out of the retained window. See `docs/streaming-sequencing.md`.

**`NotificationEvent` variants** (defined in `broadcast/notification.rs`):

| Variant | Sent to | Trigger |
|---|---|---|
| `ChatMessage { message, room_preview_text }` | all room members | new message |
| `RoomChangeEvent { message, room_preview_text }` | all room members | join/leave/invite |
| `NewRoom { room, created_by }` | invited user | room creation / invite |
| `LeaveRoom { room_id }` | leaving user | user leaves room |
| `FriendRequestReceived { from_user }` | target user | friend request sent |
| `FriendRequestAccepted { from_user }` | requester | request accepted |
| `UserReadChat { user_id, room_id }` | all room members | room marked as read |
| `SystemMessage { message }` | any | system-level events |
| `Resync { reason }` | one client connection | replay gap / lag — client must reload via REST (ephemeral) |

### Database Pattern

All data lives in PostgreSQL. SQLx macros provide compile-time query type-checking against `.sqlx/` metadata.

For function signatures involving transactions or shared executors, follow `docs/sqlx-executor-pattern.md` — this documents when to use `impl Executor<'_, Database = Postgres>` vs `&PgPool` vs `&mut PgTransaction`.

### Authentication

Keycloak middleware validates the JWT on every protected request (JWKS endpoint cached). Valid tokens inject `KeycloakClaims` into request extensions. Handlers extract the caller's UUID via:
```rust
Extension(claims): Extension<KeycloakClaims>
```

### Cursor Pagination (`core/cursor.rs`)

**All list endpoints use cursor pagination — no `page`/`pageSize` parameters.**

Cursors are base64url-encoded JSON structs. The generic infrastructure:
```rust
CursorResults<T> { next_cursor: Option<String>, content: Vec<T> }
decode_cursor::<MyCursor>(base64_str) -> Result<MyCursor, CursorError>
encode_cursor(&cursor) -> Result<String, CursorError>
```

Existing cursor types:
- `UserPaginationCursor { last_seen_name, last_seen_id }` — user search via `raw_name` index
- Message timeline — timestamp-based (`created_at` DESC), efficient with indexed column

### Key Data Model Facts

**Rooms & Membership** (`chat_room_participant`):
- Tracks `MembershipStatus` per (room, user): `Joined`, `Invited`, `Left`
- Rows are never deleted — leaving sets status to `Left` to preserve history
- Most queries filter to `Joined` only
- `RoomContext` / `RoomMemberContext` are cached in Redis for fast participant lookups

**Messages** (`chat_message`):
- Stored in PostgreSQL, `msg_body` column is JSONB (`sqlx::types::Json<MessageBody>`)
- `MsgType`: `Text`, `Media`, `Reply`, `RoomChange`
- `MessageBody` variants: `TextBody`, `MediaBody`, `ReplyBody`, `RoomChangeBody`
- `RoomChangeBody` sub-types: `UserJoined`, `UserLeft`, `UserInvited`
- `latest_message_preview_text` on rooms is JSONB (`LastMessagePreviewText` enum)

**User Relationships** (`user_relationship`):
- Symmetric — stored once as (user_a_id, user_b_id) with directional state
- `RelationshipState`: `FRIEND`, `A_INVITED`, `B_INVITED`, `A_BLOCKED`, `B_BLOCKED`, `ALL_BLOCKED`
- Resolved to client-relative `Relationship`: `Friend`, `InviteSent`, `InviteReceived`, `ClientBlocked`, `ClientGotBlocked`

**Read Receipts**:
- `last_message_read_at` per (user, room) on `chat_room_participant`
- Updated via `POST /api/rooms/{id}/mark-read`; broadcast as `UserReadChat` so all user devices sync
- `allow_read_receipts` flag per participant (privacy control)

### Routing

```
GET    /health
POST   /api/rooms/create-room
GET    /api/rooms
GET    /api/rooms/search
GET    /api/rooms/{id}
GET    /api/rooms/{id}/detailed
GET    /api/rooms/{id}/users
GET    /api/rooms/{id}/timeline
POST   /api/rooms/{id}/leave
POST   /api/rooms/{id}/invite/{user_id}
POST   /api/rooms/{id}/upload-img
POST   /api/rooms/{id}/mark-read
GET    /api/rooms/{id}/read-states

POST   /api/send-msg
GET    /api/notifications
GET    /api/notifications/cursor
GET    /api/sse
ANY    /api/wss

GET    /api/users/{user_id}
GET    /api/users/search
GET    /api/users/friends
GET    /api/users/friends/requests
POST   /api/users/friends/add/{user_id}
POST   /api/users/friends/accept-request/{sender_id}
DELETE /api/users/friends/reject-request/{sender_id}
DELETE /api/users/friends/{friend_id}
POST   /api/users/ignore/{user_id}
DELETE /api/users/ignore/{user_id}
```

Middleware stack (protected routes): `TraceLayer` → `CorsLayer` → `KeycloakAuthLayer` → `DefaultBodyLimit` (5 MB) → `inject_request_path`

### Error Handling

All handlers return `Result<Json<T>, HttpError>`. `HttpError` serializes to:
```json
{ "status": 404, "errorCode": "NOT_FOUND", "message": "...", "timestamp": "...", "path": "/api/..." }
```
`path` is injected by `inject_request_path` middleware on error responses.

## Development Patterns

**New endpoint**: handler in `handler.rs` → service logic → repository query → register in `routes.rs`. No business logic in handlers.

**New SQL query**: write query with `sqlx::query!` / `sqlx::query_as!`, run `cargo sqlx prepare`, commit `.sqlx/`.

**SQLx executor signatures**: read `docs/sqlx-executor-pattern.md` before writing any repository function that needs to participate in a transaction.

**New message type**: add to `MsgType` and `MessageBody` enums in `messaging/model.rs`, handle in `message_service.rs`, update `LastMessagePreviewText` if needed for room previews.

**New broadcast event**: add variant to `NotificationEvent` in `broadcast/notification.rs`, broadcast via `BroadcastChannel::get().send_event(...)` after the DB write, update all `match` arms.

**New cursor type**: implement `Serialize + Deserialize + Default` on a struct, use `encode_cursor` / `decode_cursor` from `core/cursor.rs`, return `CursorResults<T>` from the endpoint.

**Broadcasting after writes**:
```rust
let bc = BroadcastChannel::get();
bc.send_event_to_all(member_ids, Notification::new(
    NotificationEvent::ChatMessage { message, room_preview_text },
)).await;
```

## Production Deployment

1. `docker build -t ism:latest .`
2. Mount `production.config.toml` with real credentials; set `ISM_MODE=production`
3. Run `sqlx migrate run` before starting ISM
4. Health check: `GET /health` → 200