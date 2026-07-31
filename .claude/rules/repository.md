---
paths:
  - src/**/repository.rs
---

# Repository Rules

All data lives in PostgreSQL. SQLx macros (`sqlx::query!` / `sqlx::query_as!`) provide compile-time query type-checking against `.sqlx/` metadata.

## Shape

One repository per domain, in `src/<domain>/repository.rs`, implementing `core::Repository`:

```rust
#[derive(Clone)]
pub struct RoomRepository {
    db: Database,
}

impl Repository for RoomRepository {
    fn new(db: &Database) -> Self {
        Self { db: db.clone() }
    }
}
```

- Hold **`Database` and nothing else** — no cache, no event bus, no config, no other repository.
- Return `sqlx::Error`. The service converts to `AppError` via `?`.
- A repository may be held by several services; see `.claude/rules/architecture.md`.

## Executor Signatures

Before writing any repository function that participates in a transaction, follow `.docs/sqlx-executor-pattern.md`. The three cases:

- `self.db.pool()` internally — standalone query, no transaction involvement
- `impl Executor<'_, Database = Postgres>` — caller decides whether to pass pool or transaction
- `&mut PgConnection` — must run inside a transaction the caller owns

**Repositories never begin transactions.** There is no `start_transaction()` and no
`get_connection()`; a service opens one with `Database::begin()` and passes it in. A transaction
spanning several repositories is business logic, so it belongs to the layer that knows why the
writes must be atomic.

## After Any SQL Change

Run `cargo sqlx prepare` to regenerate `.sqlx/` compile-time metadata, then commit `.sqlx/`.

## Query Conventions

- Use `sqlx::query!` for queries without a return type mapping.
- Use `sqlx::query_as!` for queries mapping to a struct.
- No N+1 queries — fetch related data in a single query or via `JOIN`.
- All indexed lookups; no full-table scans on hot paths.