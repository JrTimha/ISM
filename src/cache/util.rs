/**
 * Used to pub/sub room updates to the cache
 */
pub const CHAT_CHANNEL: &str = "chat_room:";

/**
 * Used to pub/sub room updates to the cache
 */
pub const ROOM_CONTEXT: &str = "room_context:";

/**
 * Per-user Redis Stream holding recent durable notifications for reconnect replay.
 * Entry IDs are `<seq>-0`; the stream is length-capped via `XADD ... MAXLEN ~ N`.
 */
pub const USER_NOTIFICATIONS: &str = "user_notifications:";

/**
 * Monotonic per-user sequence counter (INCR), used to order and replay durable notifications
 */
pub const USER_SEQUENCE: &str = "user_seq:";
