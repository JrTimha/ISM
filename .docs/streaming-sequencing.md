# Real-time Streaming: Envelope, Sequencing & Resync — Design

> Status: **Implemented (Phase A)**. Foundation for future streaming work
> (topic subscriptions, presence). Multi-server fan-out is explicitly out of scope.

This documents how ISM delivers real-time events over WebSocket (`/api/wss`) and
SSE (`/api/sse`) without silent loss, and how a client recovers after a
reconnect or a slow-consumer lag.

## 1. Goals

- A stable, versioned wire envelope that can evolve without breaking clients.
- A monotonic **per-user** sequence number so a client can detect gaps and
  resume exactly where it left off.
- A bounded, hybrid recovery model: replay small gaps from cache; for anything
  older than the retention window, tell the client to reload via REST. Retention
  is count-bounded (the last ~N events per user), not time-bounded.
- Ephemeral events (typing-style signals) must **not** be replayed — a typing
  indicator from 30 minutes ago is noise.

## 2. Wire Envelope

```jsonc
{
  "v": 1,                 // envelope version (NOTIFICATION_VERSION)
  "seq": 4711,            // monotonic per-user; omitted for ephemeral events / no Redis
  "type": "chatMessage",  // NotificationEvent tag
  "createdAt": "2026-...",
  ...payload              // variant fields, serde-flattened
}
```

Built only via `Notification::new(body)` (`src/broadcast/notification.rs`).
`seq` is left `None` at construction and assigned per-recipient during delivery.

## 3. Durable vs. Ephemeral

`NotificationEvent::is_ephemeral()` is the single source of truth.

- **Durable** (default, all current variants): assigned a `seq`, cached for
  replay, push-fallback when offline.
- **Ephemeral** (`Resync`, and future typing/presence): no `seq`, never cached,
  live-only. Dropped for offline users by design.

## 4. Sequencing

`Cache::next_sequence(user_id)` → Redis `INCR user_seq:{id}` (+ TTL refresh,
`SEQUENCE_TTL_SECONDS`). Returns `Option<u64>`:

- `Some(seq)` — sequencing available.
- `None` — `NoOpCache` / no Redis: events are delivered best-effort, `seq` stays
  `None`, and replay is unavailable.

Because `seq` is **per-user**, a fan-out (`send_event_to_all`) allocates a
distinct `seq` for each recipient — there is no shared sequence across users.

## 5. Caching & Replay (`src/cache/redis_cache.rs`)

- Durable notifications are appended to a per-user **Redis Stream**
  (`user_notifications:{id}`). Each entry's ID is `<seq>-0`, so the stream is
  ordered by `seq` and the entry holds the serialized notification under the
  `data` field.
- `XADD ... MAXLEN ~ STREAM_MAX_LEN` trims older entries on every write (amortized
  O(1)), bounding retention to the last ~N events. A TTL is refreshed on each
  write so a fully inactive user's stream is reclaimed — there is **no background
  cleanup task**.
- `get_notifications_since_seq(user_id, last_seq)` → `ReplayResult`:
  - `Events(vec)` — `XRANGE` from exclusive `(<last_seq>-0` to `+`, in order.
  - `ResyncNeeded` — the oldest retained `seq` is already newer than
    `last_seq + 1` (the gap was trimmed). Because the stream is a single
    structure, there is no separate index that can dangle, so this is the only
    resync trigger.

## 6. Connection Handshake (`src/messaging/notifications.rs`)

1. **Subscribe first**, then read the replay (so events produced during the
   handshake are buffered, not lost).
2. Resolve `?last_seq=<n>` via `resolve_handshake`:
   - no `last_seq` → fresh connection, no replay.
   - `Events` → send them; `high_water` = max replayed `seq`.
   - `ResyncNeeded` / error → send a single `Resync` event, `high_water = 0`.
3. Go live; drop any durable event with `seq <= high_water` (dedupes the overlap
   between replay and the live buffer). Ephemeral events always pass.
4. On `RecvError::Lagged` (slow consumer overran the 100-deep broadcast buffer),
   send a `Resync` and reset `high_water` to 0.

The REST endpoint `GET /api/notifications?last_seq=<n>` exposes the same replay
for explicit pulls; a `ResyncNeeded` surfaces as a single `Resync` element.

`GET /api/notifications/cursor` → `{ "seq": <n> }` returns the highest sequence
currently issued to the caller (0 if none yet) **without** advancing it. A client
that has just done a full REST sync uses this to seed its stored cursor.

## 7. Client Contract

There are two distinct (re)connection modes — keep them separate:

- **Short reconnect** (no state reload, e.g. a brief network blip): reconnect with
  `?last_seq=<highest seq seen>`. The server replays the small gap.
- **Full REST sync** (cold start, post-`Resync`, or multi-device divergence where a
  stale `seq` would replay events the snapshot already contains): connect to the
  stream **without** any `last_seq` parameter. A fresh connection does no replay
  and streams only events from subscription onward, so there is no flood of
  already-applied events. **Subscribe before you snapshot**: open the stream first
  (buffering live events), then issue the REST calls — the snapshot is then strictly
  newer than the stream start, so any event produced in between arrives live and is
  reconciled by idempotent application, closing the snapshot/stream race. Seed the
  stored cursor from `GET /api/notifications/cursor` (or the `seq` of the first live
  event) for subsequent short reconnects.

Why this split matters: `seq` is **per-user**, shared across a user's devices. A
device returning with a `seq` from before another device advanced the counter must
*not* replay from that old `seq` after a full REST sync — it already holds current
state. Connecting fresh avoids re-delivering events it has applied.

- Treat `seq` as the ordering/dedup key (ignore `seq <= highestSeen`).
- Apply events **idempotently** (dedup by stable IDs such as `message_id`):
  delivery is at-least-once and the replay/live windows overlap by design.
- On a `Resync` event: reload authoritative state via REST (timeline, friends,
  rooms), then reconnect **without** `last_seq` (full-sync mode above).
- Ephemeral events carry no `seq` — never use them for sync state.

## 8. Out of Scope / Next

- Topic subscriptions over the WS uplink (would let typing/presence target only
  interested connections).
- Presence — see `docs/location-presence-sharing.md`.
- Multi-server fan-out (Redis Pub/Sub backplane) — deprioritized.
