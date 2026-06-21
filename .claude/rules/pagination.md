# Cursor Pagination Rules

**All list endpoints use cursor pagination. No `page` or `pageSize` parameters anywhere in the API.**

## Infrastructure (`core/cursor.rs`)

```rust
CursorResults<T> { next_cursor: Option<String>, content: Vec<T> }
decode_cursor::<MyCursor>(base64_str) -> Result<MyCursor, CursorError>
encode_cursor(&cursor) -> Result<String, CursorError>
```

Cursors are base64url-encoded JSON structs. New cursor types must implement `Serialize + Deserialize + Default`.

## Existing Cursor Types

- `UserPaginationCursor { last_seen_name, last_seen_id }` — user search, friends list, and friend requests; keyset over `(display_name, id)`, optional name filter via the `raw_name` index
- `RoomPaginationCursor { last_seen_latest_message, last_seen_room_id }` — joined-rooms list; keyset over `(latest_message, id)` DESC, optional `ILIKE` name filter (other user for single rooms, room name for groups)
- Message timeline — timestamp-based (`created_at` DESC), indexed column. Returns `TimelinePage { messages, senders }`, where `senders` bundles the deduplicated `RoomMember`s that authored a message in the page or are the original author referenced by a reply (`reply_sender_id`); left authors still resolve from `app_user`, with null participant fields

## Page Size

- Clients may pass `limit`; the server clamps it via `clamp_page_size` (`core/cursor.rs`) to `[1, MAX_PAGE_SIZE]`, defaulting to `DEFAULT_PAGE_SIZE` (20) — never trust an unbounded client limit.
- Repositories fetch `page_size + 1` rows; `next_cursor` (`core/cursor.rs`) truncates to the page and encodes the continuation cursor from the last returned item.

## Rules

- Return `CursorResults<T>` from every list endpoint.
- The client passes `cursor` as a query parameter; omit for the first page.
- If the result set is smaller than the page limit, return `next_cursor: null`.
- Never leak internal IDs or timestamps directly — always encode them in the cursor.