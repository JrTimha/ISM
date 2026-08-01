# Request Validation Changes

> **Audience:** frontend / API clients
> **Type of change:** stricter input validation. **No response payload changed.**

The data-model refactor split database rows, JSONB column payloads, request bodies and response
bodies into separate type families (see `.claude/rules/model.md`). Every response and every
streaming envelope serializes byte-for-byte as before — `tests/wire_contract.rs` asserts that
against pinned JSON literals, so nothing needs to change on the read side.

What *did* change: validation is now enforced by the extractor rather than by whichever handler
remembered to call it. `validate()` previously ran at two call sites in the whole backend, and one
of those checked a single nested field. Requests that were silently accepted before can now come
back as `400` with `errorCode: "VALIDATION_ERROR"`.

## Newly rejected requests

| Endpoint | Field | New rule | Previously |
|---|---|---|---|
| `POST /api/v1/rooms/create-room` | `roomName` | 1–100 characters when present | unbounded |
| `POST /api/v1/rooms/create-room` | `invitedUsers` | 1–50 entries | unbounded |
| `POST /api/v1/rooms/create-room` | — | `Single` rooms need exactly 2 entries in `invitedUsers` | enforced in the service, same rejection, now also at the edge |
| `POST /api/v1/send-msg` | `msgType` | must match the shape of `msgBody` | never compared — a media body could be labelled `Text` |
| `POST /api/v1/send-msg` | `msgBody.mimeType` | at most 255 characters | unbounded |
| `POST /api/v1/send-msg` | `msgBody.altText` | at most 1000 characters | unbounded |
| `GET /api/v1/users/search` | `username` | 1–100 characters | unbounded, empty allowed |
| `GET /api/v1/users/friends`, `…/friends/requests` | `username` | 1–100 characters when present | unbounded |
| `GET /api/v1/rooms`, `GET /api/v1/rooms/share-targets` | `name` | 1–100 characters when present | unbounded |

The bodies of `POST /api/v1/send-msg` that were already validated (`text` 1–4000, `mediaUrl` 1–250,
`mediaType` 1–80, `replyText` 1–4000) are unchanged.

## Explicitly unchanged

- **Unnamed group rooms are still allowed.** `roomName` stays optional for `Group`; the joined-rooms
  query already `COALESCE`s a null name and `ShareTargetResponse.name` is nullable for exactly this
  case.
- **`?last_seq=` keeps its name** on `/api/v1/sse`, `/api/v1/wss` and `/api/v1/notifications`. It is
  snake_case while most fields are camelCase; that inconsistency is preserved deliberately.
- **`limit` is still clamped, not rejected.** A value above `MAX_PAGE_SIZE` (50) is not an error —
  the server returns 50. Omitted or `0` gives the default of 20. The clamping now happens while the
  query string is decoded rather than in each handler, but the observable behaviour is unchanged.
  A negative or non-numeric `limit` is still a `400`, as before.
- **Share-target payloads keep snake_case keys inside `target`** (`room_id`, `room_type`,
  `user_id`). serde's `rename_all` renames variants, not struct-variant fields; this is what clients
  parse today and it is pinned by a test.

## Soft-deleted users are no longer offered

A soft-deleted account (`app_user.deleted_at IS NOT NULL`) is now excluded from every endpoint that
*offers* a user:

| Endpoint | Effect |
|---|---|
| `GET /api/v1/users/{user_id}` | `404` instead of the profile |
| `GET /api/v1/users/search` | omitted from results |
| `GET /api/v1/users/friends` | omitted from the friends list |
| `GET /api/v1/users/friends/requests` | omitted from pending requests |
| `GET /api/v1/rooms/share-targets` | omitted from both the active and inactive sections |

Relationship rows pointing at a deleted account are **not** removed by ISM — another service
dissolves the friendship. The filter therefore lives on the read path, because the row outlives the
account and a client must not see the user in the meantime. Note that `friendsCount` on a profile
still counts the relationship until that service catches up, so a friends list can legitimately be
shorter than the count next to it.

Endpoints that report a user as *historical fact* deliberately still include deleted accounts:

- `GET /api/v1/rooms/{room_id}/users` and `…/read-states` — a room's participant list drives
  broadcast fan-out; dropping a member would silently stop delivering that room's events.
- `GET /api/v1/rooms/{room_id}/timeline` — the `senders` bundle must resolve every message author,
  or messages render unattributed.

## Server-side behaviour change (not visible to clients)

`chat_room.latest_message_preview_text` now stores the **full** message text. Previously the
preview type was shared between the JSONB column and the HTTP response, so the display-only
truncation (`> 50` characters → first 40 plus `...`) also ran on the write path and the column held
shortened text.

Clients still receive the truncated string — truncation moved into the storage → response
conversion. Rows written before this change are already 43 characters, below the threshold, so
re-truncating them is a no-op and existing previews render identically.
