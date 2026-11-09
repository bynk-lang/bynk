---
level: patch
changelog: "`bynk check` counts a D1 call's parameters even when its binding has no migrations directory (`bynk.d1.sql_param_count`); the other D1 SQL checks still need a schema, and `[check] d1_sql = false` turns both off"
---

## ADR: d1-sql-count-without-schema
title: A D1 call's parameter count is checked without a schema
summary: A Rust scanner counts `?`/`?N` parameters for bindings with no migrations; one opt-out key

**Context.** #3070 prepares each literal D1 statement against the schema a binding's migrations build, and takes the parameter count from the prepared statement. A binding with no `migrations/<database>/` directory has no schema, so nothing was checked (decision B), and a wrong `?N` count failed only at run time. The count is a property of the SQL text and the list, not of the schema (#3126).

**Decision.** For a binding with no migration files, the checker still reports `bynk.d1.sql_param_count` for a literal (or `const`-bound, or `let`-bound-list) statement whose parameter count differs from the list literal's length. `sql_invalid` and `select_field_missing` need a schema and are not reported. This reverses decisions B (partly) and E (the "no Rust scanner" rule): preparing against an empty database fails with `no such table` before the count is available, so a small scanner counts parameters by SQLite's rule — `?NNN` is `NNN`, a bare `?` the next number, the count the highest index — and mirrors the test tier's `sqlParameters`. Named parameters (`:a`, `@a`, `$a`) follow that baseline: each distinct name takes the next number once. An agreement test runs a corpus through the scanner and through `rusqlite`'s `parameter_count()`. A database whose migrations exist but failed is still skipped, since its `migration_invalid` warning already stands for every statement. One opt-out key: `[check] d1_sql = false` turns off the whole check, with or without migrations.

**Consequences.** Projects that build their schema some other way gain the count check. A migrated binding keeps `prepare`'s own count, so the scanner is a second rule only for unmigrated bindings; it may later replace the call for one rule. The `sqlParameters` baseline for named parameters is tied to the test tier: if that changes, the scanner and its corpus change with it.
