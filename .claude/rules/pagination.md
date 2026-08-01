# Cursor Pagination Rules

**All list endpoints use cursor pagination. No `page` or `pageSize` parameters anywhere in the API.**

## Infrastructure (`core/cursor.rs`)

```rust
CursorResults<T> { cursor: Option<String>, content: Vec<T> }
decode_cursor::<MyCursor>(base64_str) -> Result<MyCursor, CursorError>
encode_cursor(&cursor) -> Result<String, CursorError>
```

Cursors are base64url-encoded JSON structs. New cursor types must implement `Serialize + Deserialize + Default`.

## Existing Cursor Types

- `UserPaginationCursor { last_seen_name, last_seen_id }` (`users/model.rs`) — user search, friends list, and friend requests; keyset over `(display_name, id)`, optional name filter via the `raw_name` index
- `RoomPaginationCursor { last_seen_latest_message, last_seen_room_id }` — joined-rooms list; keyset over `(latest_message, id)` DESC, optional `ILIKE` name filter (other user for single rooms, room name for groups)
- Message timeline — timestamp-based (`created_at` DESC), indexed column. Returns `TimelinePageResponse { messages, senders }`, where `senders` bundles the deduplicated `RoomMemberResponse`s that authored a message in the page or are the original author referenced by a reply (`reply_sender_id`); left authors still resolve from `app_user`, with null participant fields

## Page Size

- Clients may pass `limit`. **Declare it as `PageSize` (`core/cursor.rs`), never `Option<u32>`:**

  ```rust
  #[serde(default)]
  pub limit: PageSize,     // handler reads it with params.limit.get()
  ```

  `PageSize` clamps to `[1, MAX_PAGE_SIZE]` *during deserialization*, defaulting to
  `DEFAULT_PAGE_SIZE` (20) when the parameter is absent or zero, so an unclamped value cannot reach
  a handler. The underlying `clamp_page_size` is private for exactly that reason — when it was
  public every handler called it by hand, and a new list endpoint that forgot would have passed a
  client-supplied number straight into `LIMIT $n`.

- **An oversized `limit` is clamped, not rejected.** Asking for 1000 items is a request for more
  than the server serves, not a malformed request, and clients pass a large number to mean "as many
  as I can get". A negative or non-numeric `limit` *is* malformed and comes back as `400` from the
  extractor.
- Repositories fetch `page_size + 1` rows; `next_cursor` (`core/cursor.rs`) truncates to the page and encodes the continuation cursor from the last returned item.

## Rules

- Return `CursorResults<T>` from every list endpoint.
- The client passes `cursor` as a query parameter; omit for the first page.
- If the result set is smaller than the page limit, return `cursor: null` (the field is named `cursor` on the wire; `next_cursor` is the helper that builds it).
- Never leak internal IDs or timestamps directly — always encode them in the cursor.