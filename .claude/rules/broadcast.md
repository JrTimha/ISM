---
paths:
  - src/broadcast/**
---

# Broadcast Rules

`BroadcastChannel` holds a `RwLock<HashMap<Uuid, Sender<Notification>>>` — one Tokio broadcast
channel per connected user.

**It is not a global.** `AppStateBuilder` constructs one and injects `Arc<BroadcastChannel>` into
the services and background tasks that need it. The old `OnceCell` + `BroadcastChannel::get()` is
gone: the accessor panicked when startup order was wrong, the dependency was invisible in every
constructor, and no service could be built in a test without initialising a process-wide singleton.

A background task gets its own `Arc` clone moved in at the spawn site — the ordinary Rust pattern,
and it also yields a `JoinHandle` the global could never give you:

```rust
tokio::spawn({
    let bus = bus.clone();
    async move { bus.notify(&user_id, SystemMessage { .. }).await; }
});
```

## API

Prefer the `notify*` methods: they take the **event** and build the envelope internally, so the
version field and the unset `seq` cannot be got wrong at a call site.

```rust
// services holding Arc<BroadcastChannel>
bus.notify(&user_id, FriendRequestReceived { from_user }).await;
bus.notify_all(user_ids, event).await;
bus.subscribe_to_user_events(user_id).await;   // → Receiver
bus.unsubscribe(user_id).await;

// services holding a RoomNotifier (rooms/notifier.rs) — resolves membership, then fans out
notifier.notify_room(&room_id, UserReadChat { user_id, room_id }).await?;
notifier.notify_users(explicit_ids, event).await;   // audience is not the room's membership
notifier.notify_user(&user_id, event).await;
notifier.room_context(&room_id).await?;             // cache-first participant snapshot
notifier.invalidate(&room_id).await?;               // after any membership change
```

`send_event_to_all` takes a pre-built `Notification` and is what the `notify*` methods delegate to.
Call it directly only when the envelope did not originate here and must keep its original
`createdAt` — nothing does today, so reach for `notify` / `notify_all`.

### Macros

`notify!`, `notify_user!` and `notify_room!` wrap the methods above for fire-and-forget calls. They
exist for the one thing a function cannot do: `module_path!()` and `line!()` expand at the **call
site**, so a failed broadcast is logged against the service that attempted it rather than against
`event_broadcast.rs`.

```rust
notify_room!(self.notifier, &room_id, UserReadChat { user_id, room_id });
notify!(self.bus, &receiver_id, FriendRequestReceived { from_user });
```

Use the **method** whenever the caller needs the `Result` — if a failed fan-out should abort the
request, a macro that swallows it is in the way.

## Rules

- Always broadcast **after** a successful DB write, never before.
- After a membership change, `notifier.invalidate(&room_id)` **before** broadcasting, so a listener
  that reacts by reading the room cannot race a stale snapshot.
- Never construct `Notification` directly. `Notification::new(body)` sets the envelope version and
  leaves `seq` unset (assigned per-user during delivery); the `notify*` methods do it for you.
- Delivery goes through `deliver_to_user`, which sequences and caches durable events in one atomic
  Redis call and reports whether a live connection took the event. The push fallback belongs to
  `send_event_to_all`, which collects every offline recipient of a fan-out into **one** Kafka record
  — put it nowhere else, or a large room goes back to one record per user.
- The fan-out runs recipients concurrently, bounded by `FANOUT_CONCURRENCY`. Safe because a user
  appears at most once per fan-out; anything that could deliver twice to one user in one call would
  break per-user sequence ordering.
- Push notifications are only sent for: `ChatMessage`, `FriendRequestReceived`, `NewRoom`.

## Envelope, Sequencing & Replay

Every notification is wrapped in a versioned envelope: `{ v, seq, type, createdAt, ...payload }`.

- `seq` is a **monotonic per-user** sequence, allocated by `Cache::append_notification` — one Lua script that does `INCR` + `XADD` + both `EXPIRE`s atomically. Each recipient of a fan-out gets its **own** `seq`.
- **The stream entry ID (`<seq>-0`) is the only source of `seq`**; the stored JSON omits it and the read path re-attaches it. Never write `seq` into a cached payload — two copies can disagree, and that is exactly what the single round trip removed.
- **Durable** events are sequenced and cached (per-user Redis Stream, length-capped via `XADD ... MAXLEN ~ N`) so a reconnecting client can replay. **Ephemeral** events (`NotificationEvent::is_ephemeral() == true`) get no `seq` and are never cached — they are live-only. The converse does not hold: a durable event is delivered with `seq: None` when the cache write failed.
- Without Redis (`NoOpCache`) there is no sequencing: `seq` is `None` and no replay is possible (best-effort delivery).
- On connect, SSE/WebSocket clients pass `?last_seq=<n>`; the server replays missing durable events, deduping live events with `seq <= high_water`. If the gap was trimmed out of the retained window (or a `Lagged` is hit), the server emits a `Resync` event and the client must reload state via REST. See `Cache::get_notifications_since_seq` → `ReplayResult`.

## NotificationEvent Variants

| Variant | Sent to | Trigger | Ephemeral |
|---|---|---|---|
| `ChatMessage { message, room_preview_text, sender }` | all room members | new message (`sender: RoomMemberResponse`) | no |
| `RoomChangeEvent { message, room_preview_text }` | all room members | join/leave/invite | no |
| `NewRoom { room, created_by, first_message }` | invited user | room creation / invite (`first_message`: optional, embedded on creation) | no |
| `LeaveRoom { room_id }` | leaving user | user leaves room | no |
| `FriendRequestReceived { from_user }` | target user | friend request sent | no |
| `FriendRequestAccepted { from_user }` | requester | request accepted | no |
| `UserReadChat { user_id, room_id }` | all room members | room marked as read | no |
| `SystemMessage { message }` | any | system-level events | no |
| `Resync { reason }` | one client connection | replay gap / lag — client must reload via REST | yes |

## Broadcast Pattern

```rust
// in a service holding a RoomNotifier
self.notifier
    .notify_users(
        context.member_ids(),
        NotificationEvent::ChatMessage { message, room_preview_text, sender },
    )
    .await;
```