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
- `send_event` / `send_event_to_all` automatically cache in Redis and fall back to Kafka push for offline users.
- Push notifications are only sent for: `ChatMessage`, `FriendRequestReceived`, `NewRoom`.

## NotificationEvent Variants

| Variant | Sent to | Trigger |
|---|---|---|
| `ChatMessage { message, room_preview_text }` | all room members | new message |
| `RoomChangeEvent { message, room_preview_text }` | all room members | join/leave/invite |
| `NewRoom { room, created_by }` | invited user | room creation / invite |
| `LeaveRoom { room_id }` | leaving user | user leaves room |
| `FriendRequestReceived { from_user }` | target user | friend request sent |
| `FriendRequestAccepted { from_user }` | requester | request accepted |
| `UserReadChat { user_id, room_id }` | all room members | room marked as read |
| `SystemMessage { message }` | any | system-level events |

## Broadcast Pattern

```rust
let bc = BroadcastChannel::get();
bc.send_event_to_all(member_ids, Notification {
    body: NotificationEvent::ChatMessage { message, room_preview_text },
    created_at: Utc::now(),
}).await;
```