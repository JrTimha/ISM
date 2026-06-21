
pub const MASTER_INDEX_SET: &str = "active_user_notification_indices";

/**
 * Used to pub/sub room updates to the cache
 */
pub const CHAT_CHANNEL: &str = "chat_room:";

/**
 * Used to pub/sub room updates to the cache
 */
pub const ROOM_CONTEXT: &str = "room_context:";

/**
 * Short lived notification for a user
 */
pub const NOTIFICATION: &str = "notification:";

/**
 * Set of notifications for a user
 */
pub const USER_NOTIFICATIONS: &str = "user_notifications:";

/**
 * Monotonic per-user sequence counter (INCR), used to order and replay durable notifications
 */
pub const USER_SEQUENCE: &str = "user_seq:";