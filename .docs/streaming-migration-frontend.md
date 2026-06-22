# Frontend Migration: Streaming Envelope & Sequencing (Phase A)

> **Audience:** frontend / client developers.
> **Type of change:** **breaking** wire-format change on `/api/sse`, `/api/wss`
> and `GET /api/notifications`. There is **no compatibility window** — the server
> emits the new `v: 1` envelope only. Clients must update before deploying
> against a Phase-A backend.

See `docs/streaming-sequencing.md` for the full backend design.

---

## 1. What changed (Changelog)

### Wire format — every notification is now a versioned envelope
**Before**
```jsonc
{ "type": "ChatMessage", "createdAt": "...", "message": { ... }, "roomPreviewText": { ... } }
```
**After**
```jsonc
{
  "v": 1,
  "seq": 4711,
  "type": "ChatMessage",
  "createdAt": "...",
  "message": { ... },
  "roomPreviewText": { ... }
}
```
- `v` (number) — envelope version. Currently always `1`.
- `seq` (number, **optional**) — monotonic **per-user** sequence number.
  - Present on **durable** events (chat messages, friend requests, etc.).
  - **Absent** (`undefined`) on **ephemeral** events and when the server runs
    without Redis.
- `type` — unchanged discriminator. **PascalCase** variant name
  (`"ChatMessage"`, `"NewRoom"`, `"Resync"`, …). Payload fields remain camelCase.

### New event type: `Resync`
```jsonc
{ "v": 1, "type": "Resync", "createdAt": "...", "reason": "stream lagged, please resync via REST" }
```
Sent on a single connection when the server cannot replay losslessly (gap older
than the cache window, or the connection lagged). It carries **no `seq`** and is
**never replayed**. On receipt, the client must reload authoritative state via
REST (timeline, friends, rooms) and then keep consuming live events.

### Connection handshake — `last_seq`
- `GET /api/sse?last_seq=<n>` and `ANY /api/wss?last_seq=<n>` now accept an
  **optional** `last_seq` query parameter.
- On connect the server first **replays** every durable event with `seq > n` on
  the same connection, **then** streams live events. Omit `last_seq` on a fresh
  connection (no replay).

### REST replay endpoint — parameter renamed
- `GET /api/notifications` now takes **`?last_seq=<n>`** (number, **required**)
  instead of the old `?timestamp=<iso>`.
- Returns the durable events with `seq > n`. If the history is no longer
  available, it returns a single-element array containing a `Resync` event.

### Behavioural note
- Duplicate suppression is the client's job: the same `seq` may briefly arrive
  via both replay and live stream around reconnect. **Dedup/ignore any event
  whose `seq <= highestSeqSeen`.**

---

## 2. Frontend TODO checklist

### Parsing
- [ ] Update the notification type/model to include `v: number` and
      `seq?: number`.
- [ ] Add a handler for the new `type: "Resync"` event.
- [ ] Treat `seq` as optional — never assume it exists (ephemeral events / no
      Redis).

### Sequence tracking
- [ ] Persist `highestSeqSeen` per user (survive app restarts; e.g.
      localStorage / secure storage).
- [ ] On every received event with a `seq`: if `seq <= highestSeqSeen`, **drop
      it** (duplicate); otherwise process it and set
      `highestSeqSeen = seq`.
- [ ] Do **not** update `highestSeqSeen` from events without a `seq`.

### Connecting / reconnecting
- [ ] **After any full REST sync** (first connect, cold start, post-`Resync`, or
      whenever you (re)load rooms/friends/timeline): connect **without**
      `last_seq`. The snapshot is already authoritative, so a fresh connection
      avoids replaying events you have applied — important for multi-device, where
      `seq` is shared across devices and a stale stored value would otherwise
      flood you with already-synced events.
- [ ] **Order matters — subscribe before you snapshot.** Open the stream first
      (fresh, no `last_seq`), and only **then** issue the REST sync calls. This
      closes the gap where an event produced between the snapshot and the
      subscription would otherwise be missed: with the stream open first, the
      snapshot is strictly newer than the stream start, so anything in between
      arrives live and is reconciled by idempotent application. Buffer live events
      that arrive while the snapshot request is still in flight, then apply them
      after the snapshot.
- [ ] Seed `highestSeqSeen` after a full sync via
      `GET /api/notifications/cursor` → `{ seq }` (or from the first live event's
      `seq`).
- [ ] **Short reconnect only** (brief blip, no state reload): connect with
      `?last_seq=<highestSeqSeen>` to replay the small gap.
- [ ] Apply events idempotently (dedup by stable IDs, e.g. `message_id`):
      delivery is at-least-once.
- [ ] Keep the existing WebSocket ping/pong + keep-alive handling (unchanged).

### Resync handling
- [ ] On a `Resync` event (from the stream **or** as the REST response element),
      follow the subscribe-before-snapshot order:
  1. (Re)connect the stream **without** `last_seq` (full-sync mode) and start
     buffering live events.
  2. Re-fetch authoritative state via REST (timeline / friends / rooms).
  3. Re-seed `highestSeqSeen` (via `/api/notifications/cursor` or the first live
     event), then apply the buffered live events idempotently and continue
     consuming normally.

### REST endpoint
- [ ] Replace `GET /api/notifications?timestamp=...` calls with
      `GET /api/notifications?last_seq=<highestSeqSeen>` (use `0` to request
      everything still retained).
- [ ] Handle a returned `Resync` element the same way as a streamed one.

### Cleanup
- [ ] Remove any timestamp-based catch-up logic that relied on the old
      `?timestamp=` parameter.

---

## 3. Quick reference

| Concern | Old | New |
|---|---|---|
| Envelope | `{ type, createdAt, ...payload }` | `{ v, seq?, type, createdAt, ...payload }` |
| Catch-up cursor | `createdAt` timestamp | per-user `seq` |
| Stream handshake | — | `?last_seq=<n>` (optional) on `/api/sse`, `/api/wss`; omit after a full REST sync |
| REST replay | `GET /api/notifications?timestamp=<iso>` | `GET /api/notifications?last_seq=<n>` |
| Cursor seed | — | `GET /api/notifications/cursor` → `{ seq }` |
| Gap signal | none (silent loss) | `Resync` event → reload via REST |
| Dedup key | — | `seq` (ignore `<= highestSeqSeen`) |

---

## 4. Edge cases

- **Server without Redis:** all events arrive with `seq` absent and there is no
  replay. Clients still work live-only; reconnect simply resumes from now.
- **Ephemeral events:** never have `seq`, never replayed (e.g. `Resync`, and
  future typing/presence signals). Render them transiently; never use them for
  sync state.
- **`last_seq` too old:** instead of a partial/incorrect replay the server sends
  `Resync` — always handle it, do not assume a replay always returns events.
