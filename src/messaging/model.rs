//! Types the messaging domain shares across boundaries.
//!
//! [`MsgType`] is simultaneously a Postgres enum value, a request field and a response field, so it
//! belongs to none of `entity.rs`, `request.rs` or `response.rs` alone.

use serde::{Deserialize, Serialize};

/// The kind of a message, stored in `chat_message.msg_type`.
///
/// The only real Postgres `ENUM` in the schema — every other constrained column is a `varchar`
/// with a `CHECK`. Because the type name is shared with the database, renaming a variant is a
/// migration, not an edit.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, sqlx::Type)]
#[sqlx(type_name = "msg_type")]
pub enum MsgType {
    Text,
    Media,
    RoomChange,
    Reply,
}
