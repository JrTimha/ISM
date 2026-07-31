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

**Example: `update_user_read_status`**
- Called with `self.db.pool()` in `RoomService::mark_room_as_read` (a single write, no transaction needed)
- Available to a transactional caller as `&mut *tx` without a second signature

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

## Who Opens the Transaction

The **service**, never the repository:

```rust
let mut tx = self.db.begin().await?;                       // core::Database
self.chats.insert_message(&mut *tx, &entity).await?;       // Variant 1
self.rooms.apply_message_to_room(&mut tx, ...).await?;     // Variant 2
tx.commit().await?;
```

A transaction that spans `ChatRepository` and `RoomRepository` is a statement about the use case —
these two writes must land together — so it belongs to the layer that knows the use case.
Repositories therefore expose neither `start_transaction()` nor `get_connection()`: the pool has
exactly one owner, `Database`, and a repository that needs to run outside a transaction uses
`self.db.pool()` internally.

## Practical Notes

- `Transaction<'_, Postgres>` implements `Deref<Target = PgConnection>`, so both `&mut tx` and
  `&mut *tx` satisfy `&mut PgConnection`.
- `&Pool<Postgres>` implements `Executor<'_, Database = Postgres>`, so it works with Variant 1 but not Variant 2.
- `Database::pool()` returns the `&Pool<Postgres>` to pass to a Variant 1 function outside a
  transaction. It is on `Database` — the pool's owner — and not on any repository.
