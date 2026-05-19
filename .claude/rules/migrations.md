---
paths:
  - migrations/**
  - .sqlx/**
---

# Migration Rules

## Workflow

1. `sqlx migrate add <name>` — creates `migrations/<timestamp>_<name>.up.sql` + `.down.sql`
2. Implement SQL in the generated files
3. `sqlx migrate run` — applies pending migrations
4. `cargo sqlx prepare` — regenerates `.sqlx/` compile-time metadata
5. Commit `.sqlx/` — required so CI can build without a live database

## Conventions

- Migration names in `snake_case`.
- Always write a corresponding `.down.sql` that fully reverses the `.up.sql`.
- Set `DATABASE_URL` in `.env` before running sqlx CLI commands.
- PostgreSQL is the only database — no cross-DB compatibility needed.