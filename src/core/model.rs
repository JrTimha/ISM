//! The four kinds of data type a domain declares, and the boundary each one may cross.
//!
//! Like [`Repository`](crate::core::Repository) and [`Service`](crate::core::Service) these are
//! *convention* traits: no `dyn`, no `async_trait`, no runtime cost. Their job is to make one type
//! taxonomy compiler-visible, so a new domain cannot quietly grow a struct that is a database row
//! and an API response at the same time.
//!
//! That combination is what this module exists to prevent. Before the split, one `User` struct was
//! a `FromRow` target, a response body and a notification payload; the row could not read
//! `deleted_at` because reading it would have published it. `LastMessagePreviewText` was both the
//! JSONB column type and the response type, so a `#[serde(serialize_with = "…")]` added for display
//! purposes also ran on the `INSERT` path and truncated the value that got stored.
//!
//! | Trait | Suffix | serde | Crosses |
//! |---|---|---|---|
//! | [`DbRow`] | `…Row` | **none** | nothing — process-internal |
//! | [`JsonColumn`] | `…Json` | `Serialize + Deserialize` | the database, as the column's storage format |
//! | [`ApiRequest`] | `…Request` | `Deserialize` + `Validate` | inbound HTTP |
//! | [`ApiResponse`] | `…Response` | `Serialize` | outbound HTTP and the streaming envelope |
//!
//! Types that genuinely belong to none of these — cursors, enums shared by a column and the wire,
//! cache shapes — live in the domain's `model.rs` and implement nothing.
//!
//! See `.claude/rules/model.md`.

use serde::Serialize;
use serde::de::DeserializeOwned;
use validator::Validate;

/// A row produced by a query: the shape of `SELECT`, not the shape of an endpoint.
///
/// # Contract
///
/// - **Never derives `Serialize`.** A row that can serialize is a row whose every column is one
///   `Json(...)` away from being public, and the reviewer who adds the column is not the one who
///   notices. Rows are free to carry `deleted_at`, `raw_name`, internal counters and audit columns
///   precisely because they cannot leave the process.
/// - Converts to a response through an explicit `impl From<XRow> for XResponse` — or a named
///   constructor when the conversion needs context the `From` signature has nowhere to put, such as
///   the viewer whose perspective decides how a relationship reads.
/// - Lives in `<domain>/entity.rs`.
///
/// The `#[cfg(test)] mod convention_guards` block at the bottom of each `entity.rs` asserts the
/// absence of `Serialize` for every row in that domain, so an accidental derive fails `cargo test`
/// rather than shipping.
///
/// ```ignore
/// #[derive(sqlx::FromRow, Debug, Clone)]
/// pub struct UserRow {
///     pub id: Uuid,
///     pub display_name: String,
///     pub deleted_at: Option<DateTime<Utc>>,   // backend-only, and stays that way
/// }
///
/// impl DbRow for UserRow {}
/// ```
pub trait DbRow: Send + Sync + 'static {}

/// A value stored in a `jsonb` column.
///
/// # Contract
///
/// - **Its serde impl is a storage format, not an API.** Renaming a field, changing a tag or adding
///   a `serialize_with` stops every row already written from decoding — a failure that surfaces as
///   a `sqlx::Error` on read, long after the change. To change what a client sees, change the
///   corresponding [`ApiResponse`] instead; that is the entire reason the two are separate types.
/// - Carries no presentation logic. Truncation, redaction and formatting belong in the
///   `From<XJson> for XResponse` conversion, where they affect the response and nothing else.
/// - Lives in `<domain>/entity.rs`, next to the row that owns the column.
///
/// `tests/wire_contract.rs` pins the stored shape of every variant, so a change to one is a failing
/// assertion rather than a silent data-loss bug.
pub trait JsonColumn: Serialize + DeserializeOwned + Send + Sync + 'static {}

/// A body or query string supplied by a client.
///
/// # Contract
///
/// - **`Validate` is a supertrait, so validation is not optional.** Every request type must have
///   rules; a type with nothing to check writes `#[derive(Validate)]` and says so explicitly. This
///   is the bound that makes the missing-validation bug a compile error: before it existed,
///   `validate()` was called at two sites in the whole codebase and `NewRoomRequest` accepted an
///   unbounded `room_name` and an uncapped `invited_users`.
/// - Handlers extract it with [`ValidatedJson`](crate::core::ValidatedJson) or
///   [`ValidatedQuery`](crate::core::ValidatedQuery), never bare `Json<T>` / `Query<T>` — the
///   extractor runs `validate()`, so no call site can forget to.
/// - **Never derives `Serialize`.** ISM does not send requests; a request type that can serialize is
///   a response type wearing the wrong name.
/// - Validates *syntax* only. "Is this user in this room?" needs a database read and belongs to the
///   service — see `.claude/rules/handlers.md`.
/// - Lives in `<domain>/request.rs`.
pub trait ApiRequest: DeserializeOwned + Validate + Send + 'static {}

/// A body sent to a client.
///
/// # Contract
///
/// - The only type family allowed inside `Json<...>` and inside a
///   [`NotificationEvent`](crate::broadcast::NotificationEvent) payload.
/// - Free to evolve — that freedom is what the split buys. Renaming a field here is an API change
///   with a migration note; renaming a field on a [`JsonColumn`] is a data-loss bug.
/// - **`Deserialize` has exactly one legitimate reason:** notification payloads round-trip through
///   the Redis replay stream, so the response types embedded in `NotificationEvent` must decode
///   again. Never add it to satisfy a test — construct the value instead.
/// - Lives in `<domain>/response.rs`.
pub trait ApiResponse: Serialize + Send + 'static {}

#[cfg(test)]
mod convention_guard_mechanism {
    //! Proves the guard the domains rely on actually detects what it claims to.
    //!
    //! Every `entity.rs` ends in a `convention_guards` module asserting `!impls!(XRow: Serialize)`.
    //! Rust has no negative trait bounds, so that assertion is resolved by autoref specialization
    //! inside `impls!` — a mechanism subtle enough that a silently-always-false result would make
    //! every guard in the project vacuous while still passing. These two cases pin both directions,
    //! so a guard that stops working fails here first.

    use impls::impls;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Serializable {
        _field: u8,
    }

    struct NotSerializable {
        _field: u8,
    }

    const _: () = assert!(impls!(Serializable: Serialize));
    const _: () = assert!(!impls!(NotSerializable: Serialize));

    #[test]
    fn guard_detects_both_directions() {
        // The `const` assertions above are the real check — they fail at compile time. This test
        // exists so the module is not dead code and the intent shows up in `cargo test` output.
        assert!(impls!(Serializable: Serialize));
        assert!(!impls!(NotSerializable: Serialize));
    }
}
