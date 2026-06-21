# Online Presence & Friend Location Sharing — Design

> Status: **Planning / not yet implemented**. This document captures the agreed
> architecture for two related features:
>
> 1. **Online presence** — show friends whether a user is currently online.
> 2. **Friend location sharing** — let friends see each other on a map within a
>    50 km radius, opt-in and toggleable, near-real-time (not live).
>
> Both features are **Redis-backed and ephemeral**. No location history is
> persisted. PostgreSQL stores only the durable per-user *settings* (toggles),
> not positions.

---

## 1. Goals & Constraints

| # | Requirement | Decision |
|---|---|---|
| G1 | Online status | `online == active WebSocket connection`, tracked in Redis with TTL + heartbeat so brief reconnects don't flap. Visible to **all** friends, independent of the location feature. |
| G2 | Location visibility model | **Reciprocal**: only a user who is actively sharing may see sharing friends. A user sees friends within **50 km of their own current position**. |
| G3 | Update flow | **Client pushes** its position periodically (e.g. every few minutes / on significant movement) via REST. Friends **pull on demand** when they open the map. No live push. |
| G4 | Persistence & precision | **Redis only**, per-position TTL (~15–30 min). Coordinates **deterministically snapped to a grid** before being exposed to friends. No history, no exact coordinates leave the server. |
| G5 | Privacy | Friendship is verified server-side on every read. The 50 km filter is **server-side only** — raw friend coordinates are never sent to a client for client-side filtering. |
| G6 | Optional dependency | Redis is optional in ISM (`NoOpCache` fallback). Both features therefore **require Redis** and degrade to *"feature unavailable"* when only `NoOpCache` is active. |

### Non-goals (current phase)
- No live streaming of moving dots on a map.
- No location history / breadcrumb trail.
- No background tracking when the app is closed (a client concern, but the
  backend never assumes continuous updates).

---

## 2. Privacy Threat Model (must read)

The headline risk for any "friends within X km" feature is **trilateration**.
If an attacker can repeatedly query *"is friend F within 50 km of point P?"*
while varying `P` (by spoofing their own position), they can triangulate F's
exact location even though no exact coordinate is ever returned.

Mitigations baked into this design:

1. **Deterministic grid snapping.** A friend's position is snapped to a fixed
   grid cell (e.g. geohash precision 6 ≈ 1.2 km, or `round(lat/lng, 2)` ≈ 1.1 km)
   **before** storage/exposure. Snapping is deterministic, not random per
   request — random jitter would average out over many queries and defeats the
   purpose. The same underlying position always yields the same cell.
2. **Reciprocity.** A non-sharing user cannot query at all, removing the "free
   sensor" capability for users who don't expose themselves.
3. **Short TTL.** Stale positions disappear quickly, limiting how long a target
   can be probed at a fixed location.
4. **Coarse boundary.** Only a binary "within 50 km" + snapped pin is exposed,
   never a precise distance.

Residual risk: a determined attacker who is themselves a sharing friend can
still narrow a target to ~1 grid cell near the 50 km boundary. This is
acceptable for a social "friends nearby" feature; it is documented so the
trade-off is explicit. If stronger guarantees are ever needed, escalate to
larger cells or rate-limit position-change frequency.

---

## 3. Data Model

### 3.1 PostgreSQL (durable settings only)

Add two boolean flags to the user record (or a dedicated `user_settings` table):

| Field | Type | Default | Meaning |
|---|---|---|---|
| `share_location` | `bool` | `false` | User opts in to location sharing (G2). |
| `show_online_status` | `bool` | `true` | (Optional, future) hide presence while online. Default visible. |

These are the **only** durable additions. Positions and live presence live
exclusively in Redis.

### 3.2 Redis keys

| Key | Type | TTL | Purpose |
|---|---|---|---|
| `presence:{user_id}` | string/int (refcount) or SET of connection ids | ~60 s, refreshed by heartbeat | Online presence. Multi-device safe (see §4.1). |
| `geo:friends` | GEO sorted set | — (no per-member TTL, see note) | All sharing users' **snapped** positions. `GEOADD geo:friends lng lat {user_id}`. |
| `loc:fresh:{user_id}` | string | ~15–30 min | Freshness companion for `geo:friends`. Existence == position is fresh. |

> **Important Redis quirk:** GEO sets are sorted sets and entries **do not
> expire individually**. We therefore pair each `geo:friends` member with a
> short-TTL `loc:fresh:{user_id}` key. On read we drop (and opportunistically
> `ZREM`) any candidate whose freshness key has expired. A periodic sweep task
> garbage-collects orphaned GEO members. (Note: the notification cache used to
> need such a sweep but was migrated to a Redis Stream that self-trims via
> `XADD ... MAXLEN`; GEO sets have no stream equivalent, so a sweep is still
> required here.)

---

## 4. Component Design

### 4.1 Online Presence

Tracked in the **WebSocket lifecycle** (`broadcast/` + the `wss` handler):

- **On WS connect:** add the connection to presence. Because a user may have
  multiple devices (already true for read-receipt sync), use either:
  - a per-user **refcount** (`INCR presence:{user_id}`, `EXPIRE` on heartbeat,
    `DECR` on disconnect), or
  - a **SET of connection ids** with the key carrying a TTL refreshed on
    heartbeat.
  The SET approach is more robust against missed `DECR`s on crashes.
- **Heartbeat:** the existing WS ping/pong refreshes the TTL. If all devices die
  without a clean close, the key expires and the user goes offline naturally.
- **On clean disconnect:** remove the connection; if it was the last one, the
  user is offline.

**Reading presence:** friends pull on demand. Given a friend id list (already
resolved from `user_relationship`, `FRIEND` state), a single pipelined
`EXISTS`/`MGET` over `presence:{id}` returns who is online. Optionally honor
`show_online_status`.

**Optional enhancement:** broadcast a `PresenceChanged { user_id, online }`
event to friends via the existing `BroadcastChannel` when a user flips
online/offline, so open clients update without polling. This reuses the
established broadcast-after-write pattern and is *additive* — pull-on-demand
remains the source of truth.

### 4.2 Location Sharing

**Push (client → server):**
```
POST /api/location          { lat, lng }
```
1. Reject if `share_location == false` → `403` (or auto-enable, see open
   question Q1).
2. Reject if cache is `NoOpCache` → `503 FEATURE_UNAVAILABLE`.
3. **Snap** `(lat, lng)` to the grid (§2.1).
4. `GEOADD geo:friends lng lat {user_id}` + `SET loc:fresh:{user_id} 1 EX <ttl>`.

**Stop sharing (toggle off):**
```
DELETE /api/location
```
→ `ZREM geo:friends {user_id}` + `DEL loc:fresh:{user_id}` and set
`share_location = false`.

**Read (friends nearby):**
```
GET /api/location/friends
```
1. Require caller to be sharing (reciprocity, G2) and to have a fresh position;
   otherwise `409`/empty per Q2.
2. `GEOSEARCH geo:friends FROMMEMBER {caller_id} BYRADIUS 50 km ASC WITHCOORD`
   (or `FROMLONLAT` using the caller's just-pushed position).
3. Intersect candidates with the caller's **friend set** (from
   `user_relationship` / cached `RoomContext`-style lookup). Non-friends in the
   same radius are discarded.
4. Drop candidates whose `loc:fresh:{id}` has expired (and `ZREM` them).
5. Return snapped pins:
   ```json
   { "content": [ { "user_id": "...", "lat": 52.52, "lng": 13.40, "online": true } ] }
   ```
   Coordinates are already grid-snapped; pair with presence from §4.1.

> Scale note: `geo:friends` is a single global GEO set; the radius search is
> `O(log N + M)`. Intersecting with the friend set in the app layer is fine at
> the current single-server scale. If the user base grows large, shard the GEO
> set (e.g. by region/geohash prefix) — out of scope for this phase.

---

## 5. API Summary

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/api/location` | Push own (snapped) position; refreshes TTL. |
| `DELETE` | `/api/location` | Stop sharing; remove from GEO set + disable toggle. |
| `GET` | `/api/location/friends` | Reciprocal list of friends within 50 km (snapped pins + online flag). |
| `PUT` | `/api/users/me/settings` | Update `share_location` / `show_online_status`. |
| *(WS lifecycle)* | `/api/wss` | Presence add/remove + heartbeat (no new endpoint). |

All list responses follow the existing `CursorResults<T>` convention where
pagination applies (friends-nearby is bounded by the friend set, so a cursor is
optional — decide in Q3).

---

## 6. Code Touch Points

- `src/cache/redis_cache.rs` — extend the `Cache` trait with geo + presence
  methods (`geo_add`, `geo_search_radius`, `set_presence`, `clear_presence`,
  `get_presence`). `NoOpCache` returns a "feature unavailable" error / empty.
- A new sweep task for orphaned `geo:friends` members (the old
  `src/cache/cache_cleanup.rs` was removed when notifications moved to a
  self-trimming Redis Stream; reintroduce a dedicated task for GEO).
- WebSocket handler (`broadcast/` / `wss`) — wire presence into connect /
  heartbeat / disconnect.
- New `location` module (handler → service → cache), following the existing
  handler → service → repository layering. No `unwrap()` in production paths.
- `broadcast/notification.rs` — *(optional)* add `PresenceChanged` variant and
  update all match arms.
- Migration — add `share_location` / `show_online_status` columns; run
  `cargo sqlx prepare` after touching queries.
- `core/config.rs` — *(optional)* make grid precision / radius / TTLs
  configurable instead of hard-coded constants.

---

## 7. Why this architecture makes sense

- **Redis is the right store**: presence and live positions are ephemeral,
  high-churn, and tolerant of loss on restart — exactly Redis's sweet spot. Its
  built-in `GEOADD` / `GEOSEARCH` gives the 50 km radius query for free, with no
  schema or index work in PostgreSQL.
- **PostgreSQL stays the source of truth** for durable data (settings,
  relationships) — consistent with ISM's "PostgreSQL is the single source of
  truth" principle. Positions are deliberately *not* durable.
- **Pull-on-demand + periodic push** keeps battery and traffic low and avoids
  coupling the feature tightly to the WebSocket layer, while presence naturally
  reuses the connection ISM already maintains.
- **Privacy is designed in, not bolted on**: reciprocity + deterministic
  snapping + short TTL + server-side radius filtering directly counter the known
  trilateration attack.

---

## 8. Open Questions (resolve before implementation)

- **Q1** — Should `POST /api/location` auto-enable `share_location`, or strictly
  require the toggle to be set first via settings? (Leaning: require explicit
  opt-in via settings; push only refreshes.)
- **Q2** — Exact behavior when the caller is online but has *no fresh position*
  (e.g. just toggled on, hasn't pushed yet): empty list vs `409`?
- **Q3** — Does `GET /api/location/friends` need a cursor, or is it always small
  enough to return whole (bounded by friend count)?
- **Q4** — Heartbeat interval & presence TTL values (e.g. 30 s ping / 60 s TTL)
  and position TTL (15 vs 30 min) — tune against client behavior.
- **Q5** — Grid precision: geohash-6 (~1.2 km) vs `round(2)` (~1.1 km) vs a
  configurable value. Affects the privacy/usefulness trade-off.