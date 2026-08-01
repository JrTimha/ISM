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

**The layer contract is `.claude/rules/architecture.md` — read it before adding a domain, a
service, or a repository.** Summary:

```
Routes (router.rs)
  ↓ Keycloak JWT middleware → injects KeycloakToken into request extensions
Handlers (<domain>/handler.rs)          State<XService> via FromRef; syntax validation only
  ↓
Services (<domain>/service.rs | service/)  Clone structs, constructor-injected dependencies
  ↓
Repositories (<domain>/repository.rs)    hold core::Database, nothing else ─── PostgreSQL (SQLx)

Arc<BroadcastChannel> ── injected into services and background tasks (no global)
```

Per-domain file layout is fixed: `mod.rs`, `routes.rs`, `handler.rs`, `repository.rs`,
`entity.rs`, `request.rs`, `response.rs`, `model.rs`, and `service.rs` (single service) or
`service/` (several).

### Data model taxonomy

**The contract is `.claude/rules/model.md` — read it before adding any type.** Every type belongs to
exactly one of four categories, and the category decides which boundary it may cross:

| Category | Suffix | serde | File |
|---|---|---|---|
| Database row | `…Row` | **none** | `entity.rs` |
| JSONB column payload | `…Json` | `Serialize + Deserialize` (this *is* the storage format) | `entity.rs` |
| Request body | `…Request` | `Deserialize` + `Validate` | `request.rs` |
| Query string | `…Query` | `Deserialize` + `Validate` | `request.rs` |
| Response body | `…Response` | `Serialize` | `response.rs` |

Cursors, enums that are both a column value and a wire value (`RoomType`, `MsgType`,
`RelationshipState`), and cache shapes live in `model.rs`.

Four marker traits in `core/model.rs` pin this: `DbRow`, `JsonColumn`, `ApiRequest`, `ApiResponse`.
`ApiRequest: DeserializeOwned + Validate` is the load-bearing one — a request type without
validation rules cannot be registered, so handlers extract with `ValidatedJson<T>` /
`ValidatedQuery<T>` (`core/extract.rs`) and validation cannot be forgotten at a call site.

Rows carry backend-only columns precisely because they cannot serialize — `UserRow` holds `email`,
`created_at`, `deleted_at`, `last_modified_at` and `raw_name`, none of which reach a response. Each
`entity.rs` ends in a `#[cfg(test)] mod convention_guards` that asserts the absence of `Serialize`
via `impls!`, so an accidental derive fails `cargo test`.

Conversions are `From` impls (`impl From<UserRow> for UserProfileResponse`), or a named constructor
when the conversion needs context `From` cannot carry (`Relationship::for_viewer(&row, viewer_id)`).

Two convention traits in `core/traits.rs` — `Repository` and `Service` — pin the construction shape
project-wide. They are static: no `dyn`, no `async_trait`. There are deliberately **no**
`trait UserService` / `trait UserRepository` abstractions; repositories are tested with
`#[sqlx::test]` against a real database, and services are constructible in tests because every
dependency arrives through `new(...)`.

### AppState (`core/app_state.rs`)

Holds **services only**, built once by `AppStateBuilder` and shared as `Arc<AppState>`.

| Field | Type |
|---|---|
| `env` | `ISMConfig` |
| `room_service` | `RoomService` |
| `share_service` | `ShareService` |
| `timeline_service` | `TimelineService` |
| `message_service` | `MessageService` |
| `notification_service` | `NotificationService` |
| `user_service` | `UserService` |

Handlers never receive `AppState`. Each service has a `FromRef<Arc<AppState>>` impl, so a handler
takes `State<RoomService>` and its signature states exactly what it can reach.

### Composition root (`core/builder.rs`)

`AppStateBuilder` is the only place that constructs anything:

```rust
let Bootstrap { state, tasks } = AppStateBuilder::new(config)
    .with_cache(cache)        // optional overrides, for tests
    .build()
    .await?;                  // Result<_, StartupError> — no boot panics
```

Build order is the proof that the service graph is a DAG (Rust has no GC, so an `Arc` cycle would
leak): `Database` → `Cache` → `PushNotificationProducer` → `Arc<BroadcastChannel>` → spawn the
Redis subscriber → `ObjectStorage` → repositories → `RoomNotifier` → tier-1 services →
`UserService`.

`build()` returns `Bootstrap { state, shutdown }`. The `state` is *moved* into the router; the
`Shutdown` half — the spawned `JoinHandle`s, the `Database` and the `ShutdownController` — stays
with the caller, which is why neither has to be cloned. `Shutdown::run()` aborts the tasks, then
closes the pool — **always call it, including on the error path**. Without `Database::close()` the
pool's `Drop` only closes the client side and PostgreSQL holds the backends until its TCP keepalive
expires, which a restart loop turns into a `max_connections` outage.

**Live SSE/WebSocket connections would otherwise block shutdown forever** — axum can stop accepting
but cannot end a response body that never completes. `Shutdown::begin_when(os_signal())` is passed
to `with_graceful_shutdown`; it waits for SIGTERM/Ctrl-C and then fires a `ShutdownSignal` that both
streaming handlers select on. Any new long-lived handler **must** do the same; see
`.claude/rules/architecture.md` and `tests/graceful_shutdown.rs`.

The single PostgreSQL pool (max 20 connections) lives in `core::Database` and is shared by cloning;
`Database::begin()` is the only way to a transaction, and only services call it.

### Configuration (`core/config.rs`)

`ISMConfig::new()` takes no arguments — it reads `ISM_MODE` itself (default: `development`),
because choosing the mode *is* part of loading the configuration. Layered TOML loading:
`default.config.toml` → `{mode}.config.toml` → `ISM_*` environment variables. The resolved mode is
kept on the config as `run_mode`, so nothing else re-reads `ISM_MODE` with its own copy of the
default.

**`config.rs` is the only module that reads the environment.** Every field is reachable through the
env layer, flat ones included — `ISM_LOG_LEVEL` sets `log_level`, `ISM_CORS_ORIGIN` sets
`cors_origin` — so no other module should ever call `env::var("ISM_…")`. An env var that is set but
empty is treated as unset (`ignore_empty`), since container tooling forwards undeclared variables as
empty strings. The mapping is covered by tests in `core/config.rs`.

Tracing is deliberately **not** initialised by `ISMConfig::new()`: installing a subscriber is a
process-global side effect that panics on a second call, and a constructor that mutates global
state is the pattern this architecture removed from `AppState::new` and `RedisCache::new`. `main`
calls `init_tracing(&config.log_level)` after loading the config.

Config sections:
- `room_db_config` — PostgreSQL connection (host, port, user, password, db_name)
- `token_issuer` — Keycloak host + realm
- `object_db_config` — MinIO/S3 credentials
- `kafka_config` — Kafka bootstrap, topic, partition, consumer group
- `redis_cache_url` — optional Redis URL (omit to use `NoOpCache`)
- `use_kafka` — bool, enables Kafka push notification producer
- `cors_origin` — allowed CORS origin

Env var override format: `ISM_ROOM_DB_CONFIG__DB_HOST=...` — `__` descends into a section, a
single `_` stays part of the field name.

### Real-time Broadcasting (`broadcast/`)

`BroadcastChannel` holds a `RwLock<HashMap<Uuid, Sender<Notification>>>` — one Tokio broadcast
channel per connected user. It is **not a global**: `AppStateBuilder` builds one and injects
`Arc<BroadcastChannel>` into the services and tasks that need it. See `.claude/rules/broadcast.md`.

**API** — prefer the `notify*` methods, which take the event and build the envelope internally:
```rust
bus.notify(&user_id, FriendRequestReceived { from_user }).await;
bus.notify_all(user_ids, event).await;
bus.subscribe_to_user_events(user_id).await; // → Receiver
bus.unsubscribe(user_id).await;

// rooms/notifier.rs — resolves membership (cache-first), then fans out
notifier.notify_room(&room_id, event).await?;
notifier.notify_users(explicit_ids, event).await;
notifier.invalidate(&room_id).await?;   // after any membership change
```

`notify!` / `notify_user!` / `notify_room!` wrap these for fire-and-forget calls; they capture the
caller's `module_path!()`/`line!()` for the failure log, which a function cannot do.

**Rules**:
- Always broadcast **after** a successful DB write, never before.
- Invalidate the cached room context **before** broadcasting a membership change.
- Never construct `Notification` directly; `seq` is assigned per-user during delivery.
- `send_event` / `send_event_to_all` assign a monotonic **per-user** `seq` and cache durable events in a per-user Redis Stream (`user_notifications:{id}`, entry ID `<seq>-0`, length-capped via `XADD ... MAXLEN ~ N` — no background cleanup). Both happen in **one atomic Lua script** (`Cache::append_notification`): the entry ID must be the value `INCR` returned, which a `MULTI`/`EXEC` pipeline cannot express, and as two round trips the pair could half-succeed and burn a sequence. The stored JSON omits `seq` — the entry ID is it, re-attached on read.
- The fan-out is concurrent (`FANOUT_CONCURRENCY`, 32) and offline recipients share **one** Kafka push record, whose envelope carries no `seq`.
- **Ephemeral** events (`NotificationEvent::is_ephemeral()`) get no `seq` and are never cached — live-only (e.g. `Resync`, future typing indicators).
- Push notifications are only sent for: `ChatMessage`, `FriendRequestReceived`, `NewRoom`.
- Wire envelope: `{ v, seq, type, createdAt, ...payload }`. Clients reconnect with `?last_seq=<n>` on `/api/v1/sse` and `/api/v1/wss`; the server replays missing durable events or emits a `Resync` when the gap was trimmed out of the retained window. See `docs/streaming-sequencing.md`.

**`NotificationEvent` variants** (defined in `broadcast/notification.rs`):

| Variant | Sent to | Trigger |
|---|---|---|
| `ChatMessage { message, room_preview_text, sender }` | all room members | new message (`sender: RoomMemberResponse` so clients render a first-time sender without a lookup) |
| `RoomChangeEvent { message, room_preview_text }` | all room members | join/leave/invite |
| `NewRoom { room, created_by, first_message }` | invited user | room creation / invite (`first_message`: optional first message, embedded on creation) |
| `LeaveRoom { room_id }` | leaving user | user leaves room |
| `FriendRequestReceived { from_user }` | target user | friend request sent |
| `FriendRequestAccepted { from_user }` | requester | request accepted |
| `UserReadChat { user_id, room_id }` | all room members | room marked as read |
| `SystemMessage { message }` | any | system-level events |
| `Resync { reason }` | one client connection | replay gap / lag — client must reload via REST (ephemeral) |

### Database Pattern

All data lives in PostgreSQL. SQLx macros provide compile-time query type-checking against `.sqlx/` metadata.

For function signatures involving transactions or shared executors, follow `.docs/sqlx-executor-pattern.md` — this documents when to use `impl Executor<'_, Database = Postgres>` vs `&mut PgConnection`, and why the **service** opens the transaction (`Database::begin()`) rather than the repository.

### Authentication (`auth/`)

Keycloak middleware validates the JWT on every protected request (JWKS cached, refreshed on demand when a token fails to verify). Valid tokens inject a `KeycloakToken<AppRole>` into request extensions.

Submodules of `auth` are private; everything a caller needs is re-exported from `crate::auth` directly. Handlers take the caller as:
```rust
user: CurrentUser          // = KeycloakToken<AppRole>
```
`CurrentUser` is the whole validated token: `user.subject` (the caller's `Uuid`), `user.roles`, `user.extra.profile.preferred_username`, `user.extra.email`, plus `expires_at` / `issued_at` / `issuer` / `audience` / `authorized_party` / `jwt_id`.

**Roles**: `AppRole` (`auth/app_role.rs`) is the realm's role set — `Admin`, `User`, `LocalGuide`, and `Unknown(String)` for everything Keycloak hands out that ISM has no rule for. It is the concrete `Role` the whole app is generic over, so `<String>` appears nowhere. No route enforces a role yet; assert one in a handler with `expect_role!(&user, AppRole::Admin)` or layer-wide via `required_roles`.

See `.docs/auth.md` for the full request path, roles and custom token extractors.

### Cursor Pagination (`core/cursor.rs`)

**All list endpoints use cursor pagination — no `page`/`pageSize` parameters.**

Cursors are base64url-encoded JSON structs. The generic infrastructure:
```rust
CursorResults<T> { cursor: Option<String>, content: Vec<T> }
decode_cursor::<MyCursor>(base64_str) -> Result<MyCursor, CursorError>
encode_cursor(&cursor) -> Result<String, CursorError>
```

Existing cursor types:
- `UserPaginationCursor { last_seen_name, last_seen_id }` — user search via `raw_name` index
- Message timeline — timestamp-based (`created_at` DESC), efficient with indexed column. Returns a `TimelinePageResponse { messages, senders }`: `senders` is the deduplicated set of `RoomMemberResponse`s that authored a message in the page **or are the original author referenced by a reply** (`reply_sender_id`), resolved via `app_user LEFT JOIN chat_room_participant`, so authors who have since left still resolve, with `joined_at`/`last_message_read_at` as `null`. Combined with the `sender` on live `ChatMessage` events, the client never needs a separate sender lookup.

### Key Data Model Facts

**Rooms & Membership** (`chat_room_participant`):
- A row means the user is **currently in the room** — there is no membership state.
- Leaving **deletes** the participant row. Message history is preserved independently in `chat_message`, and sender profiles resolve from `app_user` (see Timeline below), so deleting the row loses no history.
- `RoomContext` (`Vec<RoomMemberResponse>`) is cached in Redis for fast participant lookups / broadcast fan-out.

**Messages** (`chat_message`):
- Stored in PostgreSQL, `msg_body` column is JSONB (`sqlx::types::Json<MessageBodyJson>`)
- `MsgType`: `Text`, `Media`, `Reply`, `RoomChange` — the only real Postgres `ENUM` in the schema
- `MessageBodyJson` variants: `TextJson`, `MediaJson`, `ReplyJson`, `RoomChangeJson`; the wire
  counterparts are `MessageBodyResponse` / `TextBodyResponse` / … in `messaging/response.rs`
- `RoomChangeJson` sub-types: `UserJoined`, `UserLeft`, `UserInvited`, each holding a
  `RoomMemberSnapshotJson` — a *frozen* member snapshot, deliberately not the live member type
- `latest_message_preview_text` on rooms is JSONB (`LastMessagePreviewJson`), rendered as
  `LastMessagePreviewResponse`. Long previews are truncated **in that conversion**, not on the
  write path — when one type did both jobs the display rule also ran on `INSERT` and the column
  stored pre-truncated text.

**Soft-deleted users** (`app_user.deleted_at`):
- Set by the wider Meventure platform, not by ISM. Relationship rows pointing at a deleted account
  are dissolved by **another service**, so ISM must filter on read — the row outlives the account.
- Every query that *offers* a user filters `deleted_at IS NULL`: user search, profile-by-id, friends,
  friend requests, and both halves of the share-target list.
- Queries that report a user as *historical fact* deliberately do not: `select_all_room_member`,
  `add_user_to_room` and `select_message_senders`. A room's participant list drives broadcast
  fan-out, and the timeline's `senders` bundle must resolve every message author or messages render
  unattributed.
- Consequence to expect: `friendsCount` still counts a deleted friend until the other service
  catches up, so a friends list can be shorter than the count beside it.

**User Relationships** (`user_relationship`):
- Symmetric — stored once as (user_a_id, user_b_id) with directional state
- `RelationshipState`: `FRIEND`, `A_INVITED`, `B_INVITED`, `A_BLOCKED`, `B_BLOCKED`, `ALL_BLOCKED`
- Resolved to client-relative `Relationship`: `Friend`, `InviteSent`, `InviteReceived`, `ClientBlocked`, `ClientGotBlocked`

**Read Receipts**:
- `last_message_read_at` per (user, room) on `chat_room_participant`
- Updated via `POST /api/v1/rooms/{room_id}/mark-read`; broadcast as `UserReadChat` so all user devices sync

### Routing

```
GET    /health
POST   /api/v1/rooms/create-room
GET    /api/v1/rooms
GET    /api/v1/rooms/search
GET    /api/v1/rooms/share-targets
GET    /api/v1/rooms/{room_id}
GET    /api/v1/rooms/{room_id}/detailed
GET    /api/v1/rooms/{room_id}/users
GET    /api/v1/rooms/{room_id}/timeline
POST   /api/v1/rooms/{room_id}/leave
POST   /api/v1/rooms/{room_id}/invite/{user_id}
POST   /api/v1/rooms/{room_id}/upload-img
POST   /api/v1/rooms/{room_id}/mark-read
GET    /api/v1/rooms/{room_id}/read-states

POST   /api/v1/send-msg
GET    /api/v1/notifications
GET    /api/v1/notifications/cursor
GET    /api/v1/sse
ANY    /api/v1/wss

GET    /api/v1/users/{user_id}
GET    /api/v1/users/search
GET    /api/v1/users/friends
GET    /api/v1/users/friends/requests
POST   /api/v1/users/friends/add/{user_id}
POST   /api/v1/users/friends/accept-request/{sender_id}
DELETE /api/v1/users/friends/reject-request/{sender_id}
DELETE /api/v1/users/friends/{friend_id}
POST   /api/v1/users/ignore/{user_id}
DELETE /api/v1/users/ignore/{user_id}
```

Middleware stack (protected routes): `TraceLayer` → `CorsLayer` → `KeycloakAuthLayer` → `DefaultBodyLimit` (5 MB) → `inject_request_path`

### Error Handling

All handlers return `AppResponse<Json<T>>` (= `Result<Json<T>, AppError>`, `core/errors.rs`). `AppError` serializes to:
```json
{ "timestamp": "...", "status": 404, "error": "Not Found", "message": "...", "path": "/api/v1/...", "errorCode": "CONTENT_NOT_FOUND" }
```
`path` is injected by `inject_request_path` middleware on error responses.

`AppError` variants split into **client-facing** (`Validation`, `NotFound`, `Forbidden` — message passed through) and **internal** (`Database`, `Cache`, `Serialization`, `S3`, `Processing` — logged in full, generic message returned). The auth middleware has its own `AuthError`, sanitised the same way.

## Development Patterns

**New endpoint**: handler in `handler.rs` (taking `State<XService>`) → service logic → repository query → register in `routes.rs`. No business logic in handlers. Handlers take request bodies as `ValidatedJson<T>` and query strings as `ValidatedQuery<T>` — never bare `Json<T>` / `Query<T>`.

**New service**: `Clone` struct in `<domain>/service.rs` or `<domain>/service/<name>.rs`, `impl Service`, dependencies through `new(...)`, wire it in `AppStateBuilder::build()` **after** everything it depends on, add the field to `AppState` and its `FromRef` entry.

**New repository**: `<domain>/repository.rs`, `impl Repository`, holds only `Database`, constructed in the builder with `XRepository::new(&database)`.

**New SQL query**: write query with `sqlx::query!` / `sqlx::query_as!`, run `cargo sqlx prepare`, commit `.sqlx/`.

**SQLx executor signatures**: read `.docs/sqlx-executor-pattern.md` before writing any repository function that needs to participate in a transaction.

**New message type**: add to `MsgType` in `messaging/model.rs`, add the stored variant to `MessageBodyJson` in `messaging/entity.rs` **and** its wire counterpart to `MessageBodyResponse` in `messaging/response.rs` with the `From` between them, handle it in `messaging/service/message.rs`, and update `LastMessagePreviewJson` / `LastMessagePreviewResponse` if it needs a room preview. Add a golden case to `tests/wire_contract.rs`.

**New broadcast event**: add variant to `NotificationEvent` in `broadcast/notification.rs`, broadcast through the injected bus or `RoomNotifier` after the DB write, update all `match` arms.

**New cursor type**: implement `Serialize + Deserialize + Default` on a struct, use `encode_cursor` / `decode_cursor` from `core/cursor.rs`, return `CursorResults<T>` from the endpoint. Cursors live in the domain's `model.rs`.

**Broadcasting after writes**:
```rust
self.notifier.notify_users(
    member_ids,
    NotificationEvent::ChatMessage { message, room_preview_text, sender },
).await;
```

## Production Deployment

1. `docker build -t ism:latest .`
2. Mount `production.config.toml` with real credentials; set `ISM_MODE=production`
3. Run `sqlx migrate run` before starting ISM
4. Health check: `GET /health` → 200

A failed startup exits non-zero with a `StartupError` message (unreachable database, missing S3
bucket, bad Redis URL) instead of a panic backtrace — the startup log lists each wired service by
`Service::NAME`, so a short log tells you how far boot got.