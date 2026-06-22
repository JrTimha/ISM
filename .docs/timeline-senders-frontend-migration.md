# Frontend Migration — Bundled Senders

Two wire formats changed so the client no longer needs a separate user fetch to render
messages: the **timeline** now bundles its senders, and the live **`ChatMessage`** event
embeds its sender.

## 1. Timeline response shape (`GET /api/rooms/{id}/timeline`)

Before: a bare array `MessageDto[]`. Now a `TimelinePage`:

```jsonc
{
  "messages": [ /* MessageDto[], unchanged */ ],
  "senders":  [ /* RoomMember[] */ ]
}
```

- Map over `response.messages` instead of the response directly.
- Merge `response.senders` into a local sender map (`id → RoomMember`). It contains **every
  author in this page**, including reply original authors (`replySenderId`) and authors who
  have **already left** the room. No parallel user fetch is needed for the timeline anymore.

## 2. `ChatMessage` live event carries `sender` (SSE `/api/sse` + WS `/api/wss`)

```jsonc
{ "type": "ChatMessage", "message": { /* ... */ }, "roomPreviewText": { /* ... */ }, "sender": { /* RoomMember */ } }
```

- On receive, merge `event.sender` into the same sender map. A user posting for the first
  time renders immediately, with no lookup. Also keeps names/avatars fresh, since the
  profile is always current.

## 3. `RoomMember` shape changed (affects `/users`, `/read-states`, `senders`, event `sender`)

- `membershipStatus` is **gone** — a row means "in the room". Remove any field/filter on it.
- `joinedAt` and `lastMessageReadAt` are now **nullable**: for authors in `senders` that
  have left, both come back as `null` (identity `id` / `displayName` / `profilePicture` is
  always present). Use `joinedAt == null` if you want to flag a "former member".

## Recommended pattern

Keep one sender map per room, hydrated from:

- `timeline.senders` (initial load and each scroll-back page), and
- `ChatMessage.sender` (live).

Messages reference only `senderId` / `replySenderId` — resolve them against the map.

## Unchanged

`MessageDto` itself, `roomPreviewText`, the `timestamp` query param, and all other events.
