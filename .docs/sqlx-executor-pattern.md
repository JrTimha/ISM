# SQLx Executor Pattern

This guide explains when to use the generic `Executor<'e>` trait versus an explicit `&mut PgConnection` for database functions in this codebase.

## The Two Signatures

### Variant 1 — Generic `Executor<'e>`

```rust
pub async fn insert_message<'e, E>(&self, exec: E, message: &MessageEntity) -> Result<(), Error>
where
    E: sqlx::Executor<'e, Database = Postgres>,
```

The caller can pass any of the following:

```rust
// A pool reference — sqlx acquires a connection internally
repo.insert_message(&pool, &msg).await?;

// An explicit connection
let mut conn = pool.acquire().await?;
repo.insert_message(&mut *conn, &msg).await?;

// A transaction
let mut tx = pool.begin().await?;
repo.insert_message(&mut *tx, &msg).await?;
```

### Variant 2 — Explicit `&mut PgConnection`

```rust
pub async fn apply_message_to_room(
    &self,
    conn: &mut PgConnection,
    ...
) -> Result<(), sqlx::Error>
```

The caller must pass a concrete connection. Passing `&pool` directly does not compile:

```rust
// Does NOT compile — enforced by the type system:
repo.apply_message_to_room(&pool, ...).await?;

// Works — explicit acquire:
let mut conn = pool.acquire().await?;
repo.apply_message_to_room(&mut *conn, ...).await?;

// Works — transaction (Transaction<'_, Postgres>: Deref<Target = PgConnection>):
let mut tx = pool.begin().await?;
repo.apply_message_to_room(&mut *tx, ...).await?;
```

## Which to Use and When

The decision comes down to **semantic intent**, not just flexibility.

### Use `Executor<'e>` when:

The function is called **both inside and outside of transactions** in the codebase. The extra flexibility is genuinely needed.

**Example: `insert_message`**
- Called with `&pool` in `save_room_change_message_and_broadcast` (no transaction needed)
- Called with `&mut *tx` in `send_message` (must be atomic with room state update)

### Use `&mut PgConnection` when:

The function is **always part of a larger transaction**. The restrictive type is intentional — it makes calling the function without a transaction a compile error instead of a silent consistency bug.

**Examples in this codebase:**

| Function | Why it enforces `&mut PgConnection` |
|---|---|
| `apply_message_to_room` | Must be atomic with `insert_message` |
| `update_last_room_message` | Always paired with `update_user_read_status` in a tx |
| `delete_room` | Always paired with participant cleanup |
| `remove_user_from_room` | Always paired with preview text update |

## The Core Trade-off

`Executor<'e>` is more **flexible**. `&mut PgConnection` is more **correct** for transaction-bound operations.

A future developer who tries to call `apply_message_to_room` with just `&pool` gets a **compiler error**. With a generic `Executor`, they would get a **runtime consistency bug** instead — the room state update would succeed without the message insert being part of the same atomic unit.

More options at the call site is not always better. Use the type system to enforce the invariants that matter.

## Practical Notes

- `Transaction<'_, Postgres>` implements `Deref<Target = PgConnection>`, so `&mut *tx` satisfies `&mut PgConnection`.
- `&Pool<Postgres>` implements `Executor<'_, Database = Postgres>`, so it works with Variant 1 but not Variant 2.
- When a repository function needs to expose its pool for external callers (e.g. `save_room_change_message_and_broadcast`), add a `get_connection() -> &Pool<Postgres>` method rather than making the pool field public.
