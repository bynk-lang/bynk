# 0763 — A D1 call's literal SQL is checked against the schema its migrations build

- **Status:** Accepted (v0.525)

**Context.** A D1 call's SQL is a literal (ADR 0666 B) and the schema is the
project's `migrations/` folder, yet a misspelt column, a missing table or a
wrong `?N` count appeared only when a test covered that statement, or in
production (#3070). `select[T]` writes its column list from `T` (#2865), and
nothing checked those columns existed. #3003 made the test D1 refuse a wrong
parameter count at run time.

**Decision.**

- **A. A bundled SQLite in the checker.** `bynk-check` depends on `rusqlite`
  with `bundled` (not on `wasm32`). Not Node's `node:sqlite` (`bynk check`
  needs no Node), not a SQL parser (no table or column resolution), not the
  system library (differs per machine). The baseline is that the check is never
  stricter than the test tier, which runs the same migrations on Node's SQLite;
  statements run with `DQS` off, as Node's does, so a misspelt double-quoted
  column is an error and not a string literal.
- **B. One in-memory database per D1 database, once per run.** Each migration
  file is applied in order (after wrangler's `d1_migrations` table is created),
  then every literal statement is `prepare`d (nothing runs). The binding maps to
  its migrations as the test tier does: `d1_database_name(worker, binding)`. A
  database with no `migrations/<database>/` directory is not checked.
- **C. The sites are recorded by the checker.** Each literal statement of a
  derived `D1` instance is a `Callee::D1Sql` entry under the SQL argument's id
  (for `select`/`firstAs`, the composed `SELECT <columns> <tail>`), and the unit's
  check validates them, so `bynk check` and the language server share one path.
- **D. Parameter count is SQLite's own.** `Statement::parameter_count()` is
  `sqlite3_bind_parameter_count`, the rule the test D1's `sqlParameters`
  mirrors, so the two agree by construction. The passed side counts only a list
  literal; a name, a call or a spread is not checked, never guessed.
- **E. Errors, except a failing migration.** `bynk.d1.sql_invalid`,
  `bynk.d1.sql_param_count` and `bynk.d1.select_field_missing` are errors, each
  deterministic at run time. A migration that does not apply is the warning
  `bynk.d1.migration_invalid`, and that database's statements are then not
  checked (each would echo it); the likely cause is version skew, not a broken
  file. `[check] d1_sql = false` in `bynk.toml` turns the check off, for a
  project whose SQL relies on tables created some other way.
- **F. Reported at the whole literal.** Escapes and the multi-line forms make an
  offset-to-span mapping a follow-up.

**Consequences.** `bynk check` and the editor catch a bad statement without a
test reaching it. Compiling `bynk-check` now needs a C compiler on a source
install (prebuilt releases do not). Not yet checked: `batch` and
`SqlStatement { … }` (a statement is not tied to a binding where it is built),
statements in test bodies, `let`-bound lists, the columns `all` returns against
a `Json.decodeSql[T]`, and DDL a program runs at request time (turn the check
off). A `const` SQL argument (#3065) joins by recording its site the same way.
