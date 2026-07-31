//! Business logic for the messaging domain.

mod message;
mod notification;

pub use message::MessageService;
pub use notification::{ConnectionGuard, NotificationService};
