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

`Cache::append_notification(user_id, notification)` allocates the sequence **and**
stores the event in one Lua script (`EVALSHA`): `INCR user_seq:{id}` →
`XADD user_notifications:{id} MAXLEN ~ N <seq>-0` → `EXPIRE` on both keys. Returns
`Option<u64>`:

- `Some(seq)` — sequencing available.
- `None` — `NoOpCache` / no Redis: events are delivered best-effort, `seq` stays
  `None`, and replay is unavailable.

Why a script rather than two calls or a `MULTI`/`EXEC` pipeline: the entry ID has
to *be* the value `INCR` returned, and a queued transaction has no results until it
executes. As two round trips the pair could half-succeed — a sequence allocated and
then not stored burned that number and left a hole mid-stream that no reader could
detect, because the gap check in §5 only inspects the *oldest* retained entry.

**A script is atomic in isolation, not in rollback.** Nothing interleaves with it,
but a failing command is not undone — so `XADD` must not be able to fail after
`INCR` has run, and it can: `XADD` rejects any explicit ID that is not strictly
greater than the stream's `last-generated-id`, and that value survives trimming. If
the counter is lost while the stream is not (eviction under `maxmemory` is the
realistic trigger — the counter is tiny, the stream is not), `INCR` restarts at 1
and every write for that user fails until the stream's own TTL expires. The script
therefore realigns onto `last-generated-id` when `INCR` returns 1, which costs one
extra command on a user's first write and nothing afterwards.

Because `seq` is **per-user**, a fan-out (`send_event_to_all`) allocates a
distinct `seq` for each recipient — there is no shared sequence across users.
Recipients are processed concurrently (bounded by `FANOUT_CONCURRENCY`, 32); a user
appears at most once per fan-out and the whole fan-out is awaited, so per-user
ordering is unaffected. Offline recipients are collected and pushed in **one** Kafka
record, whose envelope carries no `seq` — one envelope for many recipients has no
single correct value.

## 5. Caching & Replay (`src/cache/redis_cache.rs`)

- Durable notifications are appended to a per-user **Redis Stream**
  (`user_notifications:{id}`). Each entry's ID is `<seq>-0`, so the stream is
  ordered by `seq` and the entry holds the serialized notification under the
  `data` field.
- The stored JSON deliberately **omits `seq`**: the entry ID is it. That is what
  lets the write happen in one round trip (the payload no longer depends on the
  number being allocated), and it means the sequence has exactly one source and
  cannot disagree with itself. The read path re-attaches it from the entry ID.
  Entries written before this change carry `seq` in the payload as well; it is the
  same number, so both formats replay identically.
- `XADD ... MAXLEN ~ STREAM_MAX_LEN` trims older entries on every write (amortized
  O(1)), bounding retention to the last ~N events. A TTL is refreshed on each
  write so a fully inactive user's stream is reclaimed — there is **no background
  cleanup task**.
- `get_notifications_since_seq(user_id, last_seq)` → `ReplayResult`:
  - `Events(vec)` — `XRANGE` from exclusive `(<last_seq>-0` to `+`, in order.
  - `ResyncNeeded` — three triggers, all of them "the gap cannot be served
    losslessly":
    1. the oldest retained `seq` is newer than `last_seq + 1` — the gap was
       trimmed out of the retained window;
    2. `last_seq` is above the counter — the sequence space was reset under the
       client (TTL expiry, eviction, `FLUSH`), so its cursor references sequences
       that no longer exist;
    3. an entry cannot be decoded. Skipping it would be a silent event loss: the
       caller derives its high-water mark from what it received, so the client's
       cursor would advance past an event it never got. This goes live the moment
       the envelope format changes while a user's 24-hour stream still holds older
       entries.

## 6. Connection Handshake (`src/messaging/handler.rs` + `src/messaging/service/notification.rs`)

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
5. On **server shutdown**, close the connection: the WebSocket receives a close
   frame with code `1001` (GOING_AWAY), the SSE stream simply ends. Both
   handlers select on `NotificationService::cancelled()` — axum cannot end a
   live connection itself, so a stream that ignored this would keep the process
   alive until it was killed.

   No `Resync` is sent, and none is needed: the client reconnects with its
   stored `last_seq` and replays whatever it missed from the Redis stream, the
   same as after any other disconnect. `1001` specifically means "the server is
   going away", so a client should reconnect rather than treat it as an error —
   browser `EventSource` does this on its own.

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
- Ephemeral events carry no `seq` — but **the converse does not hold**: a missing
  `seq` does not mean the event was ephemeral. A durable event is delivered without
  one when the cache write failed, deliberately, so the client's cursor is not
  advanced past an event that never reached the stream. Read ephemerality from the
  event `type`, never from the absence of `seq`.

## 8. Out of Scope / Next

- **Redis Cluster.** `user_seq:{id}` and `user_notifications:{id}` share no hash
  tag, so they may hash to different slots and the sequencing script would be
  rejected with `CROSSSLOT`. Moving to a cluster means a key-format migration to
  `user_seq:{<uuid>}` / `user_notifications:{<uuid>}`, which invalidates every
  existing key.
- **Fan-out off the request path.** `send_message` still awaits the whole fan-out
  before responding. Two blockers before that can move into a task: `Shutdown.tasks`
  is a `build()`-local `Vec` with no accessor, so a request-time spawn cannot be
  tracked, and `Shutdown::run()` aborts rather than drains. There is also an
  ordering hazard — two independently spawned fan-outs could allocate their
  sequences in the opposite order from the send order. Measure `duration_ms` on the
  fan-out log line before deciding it is worth the machinery.
- Topic subscriptions over the WS uplink (would let typing/presence target only
  interested connections).
- Presence — see `docs/location-presence-sharing.md`.
- Multi-server fan-out (Redis Pub/Sub backplane) — deprioritized.
