//! Fire-and-forget wrappers around the broadcast methods.
//!
//! # When to use which
//!
//! Reach for the **method** ([`BroadcastChannel::notify`](crate::broadcast::BroadcastChannel::notify),
//! [`notify_all`](crate::broadcast::BroadcastChannel::notify_all),
//! [`RoomNotifier::notify_room`](crate::rooms::RoomNotifier::notify_room)) whenever the caller cares
//! about the outcome — if a failed fan-out should abort the request or change the response, you
//! need the `Result`, and a macro that swallows it is in the way.
//!
//! Reach for the **macro** when the broadcast is genuinely the last thing that happens and a
//! failure is a log line, not a control-flow decision. It exists for the one thing a function
//! cannot do: `module_path!()` and `line!()` expand at the *call site*, so the log records the
//! service that tried to broadcast rather than this file. Without it every failure would be
//! attributed to `event_broadcast.rs` and you would be back to grepping.
//!
//! ```ignore
//! // last statement of a handler-facing service method
//! notify_room!(self.notifier, &room_id, UserReadChat { user_id, room_id });
//!
//! // direct to one user
//! notify!(self.bus, &target_id, FriendRequestReceived { from_user });
//! ```
//!
//! Neither macro is an `.await`-free shortcut: both expand to an awaited call and must be used
//! inside an `async` context. They are deliberately statements, not expressions, so a value cannot
//! be silently discarded.

/// Sends an event to a single user through a [`BroadcastChannel`](crate::broadcast::BroadcastChannel).
///
/// For services that hold the bus directly. Services that own rooms hold a
/// [`RoomNotifier`](crate::rooms::RoomNotifier) instead and use [`notify_user!`].
#[macro_export]
macro_rules! notify {
    ($bus:expr, $user:expr, $event:expr $(,)?) => {{
        // `module_path!` / `line!` expand here, at the caller — the reason this is a macro.
        let __ism_origin = concat!(module_path!(), ":", line!());
        ::tracing::trace!(origin = __ism_origin, "Broadcasting event");
        $bus.notify($user, $event).await;
    }};
}

/// Sends an event to a single user through a [`RoomNotifier`](crate::rooms::RoomNotifier).
#[macro_export]
macro_rules! notify_user {
    ($notifier:expr, $user:expr, $event:expr $(,)?) => {{
        let __ism_origin = concat!(module_path!(), ":", line!());
        ::tracing::trace!(origin = __ism_origin, "Broadcasting event");
        $notifier.notify_user($user, $event).await;
    }};
}

/// Sends an event to every current member of a room, logging a failure at the call site.
///
/// Expands to an awaited [`RoomNotifier::notify_room`](crate::rooms::RoomNotifier::notify_room),
/// whose `Result` is logged rather than propagated — use the method directly if the caller needs
/// to react to a failed fan-out.
#[macro_export]
macro_rules! notify_room {
    ($notifier:expr, $room:expr, $event:expr $(,)?) => {{
        let __ism_origin = concat!(module_path!(), ":", line!());
        if let Err(error) = $notifier.notify_room($room, $event).await {
            ::tracing::error!(
                origin = __ism_origin,
                error = %error,
                "Failed to broadcast room event",
            );
        }
    }};
}
