# Share Targets — Frontend Integration Guide

The **Share Targets** feature powers a "share to chat" sheet (think Instagram's share
sheet): the user picks where to send a piece of content — an existing chat or a friend
they don't have a chat with yet — and the client delivers it there.

This endpoint **only returns the list of destinations**. Actually delivering the content
is a separate, existing call (`send-msg` or `create-room`) that depends on the kind of
target the user picked. See [Delivering content](#delivering-content-to-a-target).

---

## Endpoint

```
GET /api/rooms/share-targets
```

Protected — requires a valid Keycloak bearer token like every `/api/*` route. The list
is always scoped to the authenticated caller (their friends and their rooms).

### Query parameters

| Param    | Type     | Required | Description |
|----------|----------|----------|-------------|
| `name`   | string   | no       | Case-insensitive filter. Matches the friend's display name (1-1) or the group room name. |
| `cursor` | string   | no       | Opaque pagination cursor. Omit for the first page; pass back the `cursor` from the previous response for the next page. |
| `limit`  | integer  | no       | Page size. Clamped server-side to `[1, 50]`, defaults to `20`. Never assume the server honored your exact value. |

### Response

`200 OK` with a standard cursor-paginated envelope:

```jsonc
{
  "cursor": "eyJwaGFzZSI6ImFjdGl2ZSIsLi4u",  // pass to the next request; null = no more pages
  "content": [ /* array of ShareTarget */ ]
}
```

- `cursor: null` means you've reached the end of the list — stop paging.
- `content` is the page of share targets, already in display order (see
  [Ordering](#ordering)).

---

## The `ShareTarget` object

Every item in `content` looks like this:

```jsonc
{
  "name": "Alice",                 // string | null — see below
  "imageUrl": "https://.../a.png", // string | null — avatar (user) or room image (group)
  "target": { /* ShareTargetRef */ }
}
```

| Field      | Type            | Notes |
|------------|-----------------|-------|
| `name`     | string \| null  | Friend's display name (1-1) or the room name (group). **Can be `null`** for an unnamed group room — render a fallback (e.g. participant names or a placeholder). |
| `imageUrl` | string \| null  | Avatar / room image. `null` ⇒ render a placeholder. |
| `target`   | object          | Tells you *how* to deliver content. Discriminated by `kind`. See below. |

### `target` — the `ShareTargetRef`

This is a tagged union; **switch on `kind`**:

#### `kind: "room"` — an existing chat

The destination already exists (a group, or a friend you already have a 1-1 chat with).
Send straight into it.

```jsonc
{
  "kind": "room",
  "roomId": "550e8400-e29b-41d4-a716-446655440000",
  "roomType": "Single"   // "Single" | "Group"
}
```

#### `kind: "user"` — a friend with no chat yet

There is no room to send to yet. You must **create the 1-1 room first**, then send into
the room you get back.

```jsonc
{
  "kind": "user",
  "userId": "7b2d1f90-1c3e-4a55-9b21-0f8e2a4c1d77"
}
```

> Rule of thumb: `kind === "room"` → one call (`send-msg`).
> `kind === "user"` → two calls (`create-room`, then `send-msg` — or embed the content as
> the room's first message, see below).

---

## Ordering

The list is **two-phase**, and the server stitches both phases into a single paginated
stream — you do not need to merge anything client-side, just render `content` in order:

1. **Active targets first** — group rooms you're in and friends you already chat 1-1 with,
   sorted by most recent activity (newest first). These always come back as
   `kind: "room"`.
2. **Inactive targets after** — friends you have *no* 1-1 room with yet, sorted
   alphabetically by display name. These come back as `kind: "user"`.

A page near the boundary may contain the tail of the active section followed by the start
of the inactive section. The cursor transparently tracks which phase the next page resumes
in — just keep passing the returned `cursor` back.

Every friend appears in **exactly one** of the two sections (either you have a 1-1 room
with them or you don't), so there are no duplicates across pages.

---

## Pagination

Standard ISM cursor pagination — there are no `page`/`pageSize` params anywhere.

```
1. GET /api/rooms/share-targets?limit=20
2. render content; if response.cursor != null:
3. GET /api/rooms/share-targets?limit=20&cursor=<response.cursor>
4. repeat until cursor == null
```

The `cursor` is an opaque base64 string — do not parse, construct, or persist assumptions
about it. Combine it with `name` (the filter is re-applied on every page) and `limit`
consistently across a paging session.

---

## Delivering content to a target

Once the user taps a target, deliver the shared content based on `target.kind`.

### Existing room (`kind: "room"`)

`POST /api/send-msg` with the target's `roomId`:

```jsonc
{
  "chatRoomId": "550e8400-e29b-41d4-a716-446655440000",
  "msgType": "Media",            // "Text" | "Media" | "Reply"
  "msgBody": {
    "mediaUrl": "https://app.example.com/post/123",
    "mediaType": "link"
  }
}
```

For a plain text share use `msgType: "Text"` with `msgBody: { "text": "..." }`.

### Friend without a room (`kind: "user"`)

`POST /api/rooms/create-room`. The caller must include **their own id** in
`invitedUsers` alongside the friend. You can optionally embed the shared content as the
room's `firstMessage` so it is delivered atomically with room creation (and pushed to the
recipient in the `NewRoom` broadcast event) — no separate `send-msg` needed:

```jsonc
{
  "roomType": "Single",
  "roomName": null,
  "invitedUsers": ["<caller-id>", "7b2d1f90-1c3e-4a55-9b21-0f8e2a4c1d77"],
  "firstMessage": {               // optional; Text or Media only (never Reply)
    "mediaUrl": "https://app.example.com/post/123",
    "mediaType": "link"
  }
}
```

The response is the created `ChatRoomDto` (includes the new `id`). If you did **not** use
`firstMessage`, follow up with `POST /api/send-msg` using that `id` as `chatRoomId`.

> `firstMessage` accepts only `Text` or `Media` bodies — a brand-new room has no prior
> messages, so a `Reply` is rejected.

---

## Field validation limits (for the delivery calls)

| Field                | Limit |
|----------------------|-------|
| Text body `text`     | 1–4000 characters |
| Media `mediaUrl`     | 1–250 characters |
| Media `mediaType`    | 1–80 characters |

---

## Quick reference

| Concern              | Value |
|----------------------|-------|
| List endpoint        | `GET /api/rooms/share-targets` |
| Params               | `name?`, `cursor?`, `limit?` (clamped to 1–50, default 20) |
| Response             | `{ cursor: string\|null, content: ShareTarget[] }` |
| Deliver to room      | `POST /api/send-msg` (`kind: "room"`) |
| Deliver to friend    | `POST /api/rooms/create-room` then send, or embed `firstMessage` (`kind: "user"`) |
