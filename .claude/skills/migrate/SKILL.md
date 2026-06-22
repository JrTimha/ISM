---
name: migrate
description: Create and apply a new SQLx migration for the ISM project. Use when adding a database migration, creating tables, altering columns, or updating the schema.
disable-model-invocation: true
argument-hint: <migration-name>
allowed-tools: Bash(sqlx *) Bash(cargo sqlx prepare)
---

Create a new SQLx migration for the ISM project.

Migration name (snake_case): $ARGUMENTS

Steps:
1. Run `sqlx migrate add $ARGUMENTS` — creates a new timestamped file pair in `migrations/`
2. Show the path of the newly created migration file
3. Wait for the SQL implementation before proceeding
4. Once the migration is filled in and ready to apply:
   - Run `sqlx migrate run`
   - Run `cargo sqlx prepare` to update compile-time query metadata
   - Remind the user that `.sqlx/` must be committed