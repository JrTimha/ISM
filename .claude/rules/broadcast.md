---
paths:
  - src/broadcast/**
---

# Broadcast Rules

`BroadcastChannel` is a global singleton (`OnceCell<Arc<BroadcastChannel>>`). It holds a `RwLock<HashMap<Uuid, Sender<Notification>>>` — one Tokio broadcast channel per connected user.

## API

```rust
BroadcastChannel::get().send_event(notification, &user_id).await;
BroadcastChannel::get().send_event_to_all(user_ids, notification).await;
BroadcastChannel::get().subscribe_to_user_events(user_id).await; // → Receiver
BroadcastChannel::get().unsubscribe(user_id).await;
```

## Rules

- Always broadcast **after** a successful DB write, never before.
- Build notifications with `Notification::new(body)` — never construct the struct directly; it sets the envelope version and leaves `seq` unset (assigned per-user during delivery).
- `send_event` / `send_event_to_all` deliver to a single user via `deliver_to_user`, which assigns a per-user sequence number, caches durable events in Redis, and falls back to Kafka push for offline users.
- Push notifications are only sent for: `ChatMessage`, `FriendRequestReceived`, `NewRoom`.

## Envelope, Sequencing & Replay

Every notification is wrapped in a versioned envelope: `{ v, seq, type, createdAt, ...payload }`.

- `seq` is a **monotonic per-user** sequence (`Cache::next_sequence`, backed by Redis `INCR`). Each recipient of a fan-out gets its **own** `seq`.
- **Durable** events are sequenced and cached (per-user Redis Stream, entry ID `<seq>-0`, length-capped via `XADD ... MAXLEN ~ N`) so a reconnecting client can replay. **Ephemeral** events (`NotificationEvent::is_ephemeral() == true`) get no `seq` and are never cached — they are live-only.
- Without Redis (`NoOpCache`) there is no sequencing: `seq` is `None` and no replay is possible (best-effort delivery).
- On connect, SSE/WebSocket clients pass `?last_seq=<n>`; the server replays missing durable events, deduping live events with `seq <= high_water`. If the gap was trimmed out of the retained window (or a `Lagged` is hit), the server emits a `Resync` event and the client must reload state via REST. See `Cache::get_notifications_since_seq` → `ReplayResult`.

## NotificationEvent Variants

| Variant | Sent to | Trigger | Ephemeral |
|---|---|---|---|
| `ChatMessage { message, room_preview_text, sender }` | all room members | new message (`sender: RoomMember`) | no |
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
let bc = BroadcastChannel::get();
bc.send_event_to_all(member_ids, Notification::new(
    NotificationEvent::ChatMessage { message, room_preview_text, sender },
)).await;
```