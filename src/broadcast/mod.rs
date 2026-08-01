//! Live delivery of notifications to connected clients.
//!
//! [`BroadcastChannel`] is an ordinary value: the composition root builds one and hands
//! `Arc<BroadcastChannel>` to the services and background tasks that need it. It used to be a
//! global `OnceCell` whose accessor panicked if startup order was wrong; see
//! `.claude/rules/broadcast.md` for what replaced it and why.

mod event_broadcast;
mod macros;
mod notification;

pub use event_broadcast::BroadcastChannel;
pub use notification::{Notification, NotificationEvent};
