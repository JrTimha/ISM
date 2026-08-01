---
paths:
  - src/**/entity.rs
  - src/**/request.rs
  - src/**/response.rs
  - src/**/model.rs
---

# Data Model Rules

Every type a domain declares belongs to exactly one of four categories, and the category decides
which boundary it may cross. A type that belongs to two is the bug this file exists to prevent.

| Category | Suffix | serde | File | Crosses |
|---|---|---|---|---|
| Database row | `…Row` | **none** | `entity.rs` | nothing — process-internal |
| JSONB column payload | `…Json` | `Serialize + Deserialize` | `entity.rs` | the database, as the column's storage format |
| Request body | `…Request` | `Deserialize` + `Validate` | `request.rs` | inbound HTTP body |
| Query string | `…Query` | `Deserialize` + `Validate` | `request.rs` | inbound HTTP query string |
| Response body | `…Response` | `Serialize` | `response.rs` | outbound HTTP and the streaming envelope |

Everything else — cursors, enums that are simultaneously a column value and a wire value
(`RoomType`, `MsgType`, `RelationshipState`), cache shapes — lives in `model.rs` and implements
none of the four marker traits in `core/model.rs`.

## Rows

**Never derive `Serialize` on a `…Row`.** A row that can serialize is a row whose every column is
one `Json(...)` away from being public, and the person who adds the column is not the person who
notices. `UserRow` could not read `deleted_at`, `email` or the audit timestamps until it stopped
being a response; now it carries all of them and the `email` cannot leak, because there is no impl
that would let it.

Each `entity.rs` ends with a `#[cfg(test)] mod convention_guards` asserting exactly that:

```rust
const _: () = assert!(!impls!(UserRow: Serialize));
```

Rust has no negative trait bounds, so this cannot be a where-clause; `impls!` resolves it at compile
time instead. The mechanism is proven in both directions by `core::model::convention_guard_mechanism`
— without that, a probe that silently always answered "false" would make every guard in the project
vacuous while still passing.

Rows also **never** appear in a repository signature that names a request type. `insert_room` takes
`room_type`, `room_name` and `participants`, not `&NewRoomRequest`: a repository that knows a
request type can only ever serve the one endpoint that sends it.

## JSONB payloads

**A `…Json` type's serde impl is a storage format, not an API.** Renaming a field, changing a tag or
adding a `serialize_with` stops every row already written from decoding — a failure that surfaces as
a `sqlx::Error` on read, long after the deploy. To change what a client sees, change the
corresponding `…Response`. That is what the pair is for.

**Never put presentation logic in a `…Json` type.** `LastMessagePreviewText` used to be both the
column type and the response type, and carried a `#[serde(serialize_with = "truncate_and_serialize")]`
so long previews rendered short. Because it was one type, that display rule also ran on the `INSERT`
path, and the database spent months storing pre-truncated text. Truncation now lives in
`From<LastMessagePreviewJson> for LastMessagePreviewResponse`, where it affects the response and
nothing else.

`#[serde(untagged)]` on `MessageBodyJson` is load-bearing: the discriminator is the `msg_type`
*column*, not anything inside the JSON, so the variant is recovered by shape alone. Adding a variant
whose fields are a subset of an earlier one silently reroutes decoding.

Storage shapes that already exist are **frozen**. `RoomMemberSnapshotJson` keeps `joined_at` and
`last_message_read_at` even though they are meaningless in a frozen snapshot, because every
historical `RoomChange` message is encoded against that shape. Trimming it is a data migration, not
a struct edit.

## Requests

**`ApiRequest` has `Validate` as a supertrait, so validation is not optional.** A request type
without rules cannot be registered. Before that bound existed, `validate()` was called at two sites
in the entire codebase and `NewRoomRequest` accepted an unbounded `room_name` and an uncapped
`invited_users` list.

**Handlers take `ValidatedJson<T>` / `ValidatedQuery<T>`, never bare `Json<T>` / `Query<T>`.** The
extractor runs `validate()` before the handler body starts, so no call site can forget. A handler
that compiles has validated its input.

**`…Request` is a body; `…Query` is a query string.** Both implement `ApiRequest`, but the suffix
says which extractor the type is for, so a mismatch is visible in the signature rather than at the
first failing request. The two obey different rules: a body is camelCase like every other JSON
payload, a query string keeps whatever spelling clients already send (`?last_seq=`, `?withUser=`).
Naming them apart is what stops a blanket `rename_all` from being applied to both.

A query type may legitimately have nothing to validate — `TimelineQuery` is a single timestamp that
either parses or does not. It still writes `#[derive(Validate)]` and `impl ApiRequest`, because
"there is nothing to check here" is a claim worth making explicitly rather than a gap.

Validate *syntax* only — what is decidable from the payload alone. `check_room_cardinality` belongs
here because "a `Single` room has exactly two participants" needs no query; "these two already have
a room" stays in the service. Where both apply, both run: the request checks the raw list, the
service re-checks after blocked users are filtered out, because that filtering changes the count.

**Never derive `Serialize` on a `…Request`.** ISM does not send requests; a request type that can
serialize is a response type wearing the wrong name.

Watch `rename_all` on query-parameter types. `?last_seq=` is the documented contract for the stream
handshake, so `StreamHandshakeQuery` deliberately carries no `rename_all` — adding one would
silently rename the parameter to `lastSeq` and break every reconnecting client.

## Responses

**`Deserialize` on a `…Response` has exactly one legitimate reason:** notification payloads
round-trip through the Redis replay stream, so the response types embedded in `NotificationEvent`
must decode again. Never add it to satisfy a test — construct the value instead.

`ShareTargetRef` emits snake_case keys (`room_id`, `room_type`) inside an otherwise camelCase
response, because serde's container-level `rename_all` renames *variants* and does not reach into
struct-variant fields. That is inconsistent and it is what clients parse today;
`tests/wire_contract.rs` pins it so it cannot be "tidied" into a breaking change.

## Conversions

**Conversions are `From` impls.** No `to_dto()` inherent methods — those were what let a row decide
how it wanted to be rendered.

```rust
impl From<UserRow> for UserProfileResponse { … }        // total
impl From<LastMessagePreviewJson> for LastMessagePreviewResponse { … }
```

When the conversion needs context that `From::from` has nowhere to put, use a **named constructor**
rather than bending the signature:

```rust
Relationship::for_viewer(&relationship_row, viewer_id)
UserWithRelationshipResponse::for_viewer(&row, viewer_id)
```

`user_relationship` is stored symmetrically — `A_INVITED` says the row's `user_a_id` sent the
invite, which is meaningless without knowing who is asking — so the same row is `InviteSent` to one
participant and `InviteReceived` to the other. That is a second input, and `for_viewer` names it.

## Caches are not storage

`RoomContext` holds `RoomMemberResponse` rather than a dedicated cache struct, which looks like the
coupling this file forbids. The distinction is recoverability: a cache is a *disposable* projection,
so if the shape must change the cost is a miss and a rebuild — bump the `room_context:` prefix in
`cache::util` and old entries are ignored. A `jsonb` column has no such escape. **Reuse where a
mistake is recoverable; split where it is not.**

## Changing a wire shape

`tests/wire_contract.rs` holds a golden JSON snapshot of every response, every streaming envelope,
every stored JSONB shape and the Redis cache format. **The expected literals in that file are the
contract.** A refactor may rename the Rust types it constructs; it may not edit a literal. If an
assertion fails, the change broke a contract — fix the change.

Deliberately changing a response means editing a literal *and* writing a migration note under
`.docs/`, the way `pagination-frontend-migration.md` and `streaming-migration-frontend.md` did.
`stored_and_response_bodies_agree_today` will fail the moment a response diverges from its storage
counterpart, which is the point: divergence should be a decision, not a discovery.
