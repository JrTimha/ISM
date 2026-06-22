# Frontend Migration: Cursor Pagination for Rooms, Friends & Requests

> **Audience:** frontend / client developers.
> **Type of change:** **breaking** response-shape change on `GET /api/rooms`,
> `GET /api/users/friends`, and `GET /api/users/friends/requests`. These now
> return a paginated `CursorResults<T>` envelope instead of a plain array.
> `GET /api/users/search` is **non-breaking** (already paginated) but gains an
> optional `limit` parameter.

For the streaming/notification migration see `docs/streaming-migration-frontend.md`.
The "unknown room" section below depends on it.

---

## 1. What changed (Changelog)

### List endpoints now return a cursor envelope

All affected endpoints wrap their results in:

```jsonc
{
  "cursor": "eyJsYXN0U2Vlbk5hbWUiOiJ...",  // opaque token, or null on the last page
  "content": [ /* page items */ ]
}
```

- `content` — the items for this page (same item shape as before).
- `cursor` — opaque base64url token. Pass it back as `?cursor=` to load the
  next page. `null` means there are no more items.
- **Treat `cursor` as opaque.** Do not parse, build, or persist its contents —
  the encoding may change without notice.

### `GET /api/rooms` — paginated joined-rooms list

**Before**
```jsonc
// GET /api/rooms
[ { "id": "...", "roomName": "...", ... }, ... ]   // ALL joined rooms
```
**After**
```jsonc
// GET /api/rooms?name=&cursor=&limit=
{ "cursor": "…|null", "content": [ ChatRoomDto, … ] }
```
- New query params (all optional):
  - `name` — case-insensitive substring filter. For **single** rooms this matches
    the **other participant's** display name; for **group** rooms the room name.
  - `cursor` — continuation token from a previous `cursor`. Omit for page 1.
  - `limit` — desired page size. Server clamps to **`[1, 50]`**, default **20**.
- **Ordering:** most recent activity first (`latestMessage` DESC, `id` as
  tie-breaker). Unchanged from before, just now paged.
- `ChatRoomDto` item shape is unchanged: `id`, `roomType`, `roomImageUrl`,
  `roomName`, `createdAt`, `latestMessage`, `unread`, `latestMessagePreviewText`.

### `GET /api/users/friends` — paginated friends list

**Before**
```jsonc
[ User, … ]   // ALL friends, unordered
```
**After**
```jsonc
// GET /api/users/friends?username=&cursor=&limit=
{ "cursor": "…|null", "content": [ User, … ] }
```
- New query params (all optional): `username` (case-insensitive name filter),
  `cursor`, `limit` (`[1, 50]`, default 20).
- **Ordering:** `displayName` ASC, then `id` ASC (stable, alphabetical).

### `GET /api/users/friends/requests` — paginated incoming requests

Identical contract to `GET /api/users/friends`: same query params, same
`CursorResults<User>` response, same ordering. Returns the **incoming** friend
requests (users who invited the caller).

### `GET /api/users/search` — new optional `limit` (non-breaking)

- Already returned `CursorResults<UserWithRelationshipDto>` — response shape is
  unchanged.
- Now also accepts `?limit=` (`[1, 50]`, default 20) alongside the existing
  `username` (required) and `cursor` (optional).

---

## 2. Pagination semantics you must respect

### Page size is server-clamped
The server clamps `limit` into `[1, 50]` and defaults to `20`. **Never assume you
get exactly `limit` items back** — request `limit=1000` and you receive at most
50. Drive "has more" off `cursor`, not off `content.length`.

### Reset the cursor when the filter changes
A `cursor` is only valid for the **same** `name` / `username` it was produced
with. When the search text changes, **drop the cursor** and request page 1 again.
Keeping a stale cursor across a filter change yields inconsistent pages.

### Rooms sort by a *moving* key — dedupe and re-sort locally
Rooms are ordered by `latestMessage`, which changes whenever a new message
arrives. While you page through the list, an active room can jump to the top.
Consequences you must handle:
- The same room may appear on a page you already loaded → **dedupe by room `id`**
  when merging pages.
- A room can shift between pages while paging → don't treat "not seen yet" as
  "doesn't exist".
- On an incoming `ChatMessage`, move the room to the top of your local list
  yourself; don't rely on re-fetching to reorder.

Friends/requests sort by `displayName`, which is effectively stable — far less
churn, but the same "dedupe by `id` when merging pages" rule applies.

---

## 3. Unknown room on a notification

With the rooms list now paginated, the client typically holds only the first
page(s). A `ChatMessage` / `RoomChangeEvent` / `UserReadChat` can therefore
arrive for a room that is **not in the local cache** (an older room on a later
page, or a `NewRoom` that was missed while offline without Redis).

**Do not** expect the server to embed the full room in these events. `ChatMessage`
is a fan-out with one identical payload for all members, but a single-room
`ChatRoomDto` is **viewer-relative** (its name/image is the *other* participant) —
so the room must be fetched per client, not broadcast.

**Handling (lazy fetch + optimistic placeholder):**
1. On an event whose `chatRoomId` is unknown locally, **immediately** insert an
   optimistic placeholder from the event payload:
   - `id = message.chatRoomId`
   - `latestMessage = message.createdAt`
   - `latestMessagePreviewText = roomPreviewText`
   - `unread = true`
   This makes the room appear at the top without waiting on the network.
2. **Asynchronously** call `GET /api/rooms/{id}` (returns the viewer-relative
   `ChatRoomDto`) and replace the placeholder with the authoritative room.
3. **Dedupe in-flight fetches per `roomId`** so a burst of messages in an unknown
   room triggers a single request, not one per message.
4. `UserReadChat` carries only `roomId` — if the room is unknown, either run the
   same lazy fetch or ignore it until the room is known.

> This also self-heals the "lost `NewRoom` without Redis" case — the room is
> reconstructed on the first message rather than being missing forever.

---

## 4. Frontend TODO checklist

### Models / parsing
- [ ] Introduce a generic `CursorResults<T> = { cursor: string | null, content: T[] }`.
- [ ] Change `GET /api/rooms`, `/api/users/friends`, `/api/users/friends/requests`
      response models from `T[]` to `CursorResults<T>`.
- [ ] Treat `cursor` as an opaque string (no parsing).

### Rooms list
- [ ] Send `?limit=` and (on scroll) `?cursor=<cursor>`; stop when
      `cursor === null`.
- [ ] Wire the optional `name` filter; **reset cursor** whenever `name` changes.
- [ ] Merge pages with **dedupe by room `id`**; keep the list sorted by
      `latestMessage` DESC locally.
- [ ] On incoming `ChatMessage`, move the room to the top yourself.

### Friends & requests lists
- [ ] Same pattern: `limit` + `cursor` paging, optional `username` filter,
      cursor reset on filter change, dedupe by `id`.

### User search
- [ ] Optionally pass `limit`; response shape is unchanged.

### Unknown-room handling (see §3)
- [ ] On a notification with an unknown `chatRoomId`, insert an optimistic
      placeholder, then lazily `GET /api/rooms/{id}` and replace it.
- [ ] Dedupe concurrent fetches per `roomId`.

### Cleanup
- [ ] Remove any code assuming `GET /api/rooms` / `friends` / `requests` returns
      the **complete** set in one response.

---

## 5. Quick reference

| Endpoint | Query params | Response |
|---|---|---|
| `GET /api/rooms` | `name?`, `cursor?`, `limit?` | `CursorResults<ChatRoomDto>` |
| `GET /api/users/friends` | `username?`, `cursor?`, `limit?` | `CursorResults<User>` |
| `GET /api/users/friends/requests` | `username?`, `cursor?`, `limit?` | `CursorResults<User>` |
| `GET /api/users/search` | `username` (req), `cursor?`, `limit?` (new) | `CursorResults<UserWithRelationshipDto>` |
| `GET /api/rooms/{id}` | — | `ChatRoomDto` (viewer-relative; used for lazy room fetch) |

| Rule | Value |
|---|---|
| Default page size | 20 |
| Max page size (clamped) | 50 |
| `cursor === null` | last page, stop paging |
| Cursor validity | tied to the current filter — reset on filter change |
| Merge strategy | dedupe by item `id` |
| Rooms ordering | `latestMessage` DESC, `id` tie-break |
| Friends/requests ordering | `displayName` ASC, `id` tie-break |

---

## 6. Edge cases

- **`limit` ignored beyond 50:** requesting more returns 50; paginate for the rest.
- **Empty filter result:** `{ "cursor": null, "content": [] }` — render an
  empty state, no further paging.
- **Invalid cursor:** the server responds `400` (`Invalid Cursor-Parameters.`).
  Recover by dropping the cursor and reloading page 1.
- **Filter changed mid-scroll:** discard pending pages and any held cursor; start
  fresh from page 1.
- **Room appears twice across pages:** expected for the rooms list (moving sort
  key) — dedupe by `id`.
