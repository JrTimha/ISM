---
paths:
  - src/**/repository/**
---

# Repository Rules

All data lives in PostgreSQL. SQLx macros (`sqlx::query!` / `sqlx::query_as!`) provide compile-time query type-checking against `.sqlx/` metadata.

## Executor Signatures

Before writing any repository function that participates in a transaction, follow `docs/sqlx-executor-pattern.md`. The three cases:

- `&PgPool` — standalone query, no transaction involvement
- `impl Executor<'_, Database = Postgres>` — caller decides whether to pass pool or transaction
- `&mut PgTransaction` — must run inside a transaction the caller owns

## After Any SQL Change

Run `cargo sqlx prepare` to regenerate `.sqlx/` compile-time metadata, then commit `.sqlx/`.

## Query Conventions

- Use `sqlx::query!` for queries without a return type mapping.
- Use `sqlx::query_as!` for queries mapping to a struct.
- No N+1 queries — fetch related data in a single query or via `JOIN`.
- All indexed lookups; no full-table scans on hot paths.