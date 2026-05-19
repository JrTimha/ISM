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

- `UserPaginationCursor { last_seen_name, last_seen_id }` — user search via `raw_name` index
- Message timeline — timestamp-based (`created_at` DESC), indexed column

## Rules

- Return `CursorResults<T>` from every list endpoint.
- The client passes `cursor` as a query parameter; omit for the first page.
- If the result set is smaller than the page limit, return `next_cursor: null`.
- Never leak internal IDs or timestamps directly — always encode them in the cursor.