Add a new broadcast notification type to the ISM real-time system.

Event name: $ARGUMENTS

## Your Task

First read:
- `src/broadcast/notification.rs` — existing `NotificationEvent` variants
- `src/broadcast/mod.rs` — `BroadcastChannel` API

Then implement:

### 1. Define the new event (`src/broadcast/notification.rs`)
- Add a new variant to the `NotificationEvent` enum
- Define the corresponding payload type as a struct (with `serde::Serialize`)

### 2. Broadcast call in the service
- Show where in the service the broadcast call belongs
- Use the pattern:
  ```rust
  BroadcastChannel::broadcast(&Notification {
      user_id: Some(target_user_id),
      event: NotificationEvent::$ARGUMENTS(payload),
      timestamp: Utc::now(),
  }).await;
  ```
- Always broadcast **after** a successful DB write, never before

### 3. Final check
- Run `cargo check`
- Ensure all `match` arms on `NotificationEvent` are updated