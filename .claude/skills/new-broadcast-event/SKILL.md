---
name: new-broadcast-event
description: Add a new broadcast notification type to the ISM real-time system. Use when introducing a new SSE/WebSocket event variant that needs to be sent to connected clients.
disable-model-invocation: true
argument-hint: <EventName>
allowed-tools: Read Edit Bash(cargo check)
---

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
- Use the pattern from `BroadcastChannel::get()` — either `send_event` or `send_event_to_all`
- Always broadcast **after** a successful DB write, never before

### 3. Final check
- Run `cargo check`
- Ensure all `match` arms on `NotificationEvent` are updated