---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "`D1` databases get schema migrations: `migrations/<database>/NNNN_<name>.sql` in the project, copied beside each Worker's `wrangler.toml` and named by `migrations_dir`. `bynk deploy` applies pending ones before the push (a `d1 migrate` plan line), `bynk dev` applies them locally before serving and when one is added, and every test database starts with them applied. New diagnostics: `bynk.d1.migration_name`, `bynk.d1.migration_unreadable` and the warning `bynk.d1.migrations_unused` (standard-library P4b, [#2608](https://github.com/bynk-lang/bynk/issues/2608))"
---

## ADR: d1-migrations
title: D1 schema migrations by convention, applied by wrangler before the push
summary: Each `D1` instance's migrations live in `migrations/<database>/NNNN_<name>.sql`, are copied into the build and named by `migrations_dir`, and are applied by `wrangler d1 migrations apply` before the push, locally in `bynk dev`, and on every test database; the database records what ran, so the ledger doesn't; `D1` stays experimental, amending ADR 0662 H

**Context.** ADR 0662 shipped `D1` with an empty database: an application
created its tables with `CREATE TABLE IF NOT EXISTS` at runtime, or the
developer ran `wrangler d1 execute`, and neither evolves a schema. Wrangler
already has the mechanism. A `[[d1_databases]]` entry's `migrations_dir`
names a directory of `.sql` files, and `wrangler d1 migrations apply` runs
each one the database hasn't recorded in its own `d1_migrations` table, in
name order, each in a transaction. Three things stood in the way:

- Bynk generates `wrangler.toml` under `.bynk/`, away from the source.
- Deploy must apply before the push, non-interactively.
- Tests and `bynk dev` start from empty databases.

The state-migrations track (Gate 3) versions an agent's `store` records
through `bynk.schema.lock`. That is Bynk-owned state with Bynk-owned
semantics, and doesn't touch SQL.

**Decision.**

- **A. A convention, `migrations/<database>/NNNN_<name>.sql`.**
  - `<database>` is the instance's undecorated database name,
    `<worker>-<capability>`. It's unique per instance (the deploy refuses
    two instances that share one) and is the name the plan and the dashboard
    show.
  - A file is four digits, `_`, then letters, digits, `_` or `-`, then
    `.sql`. The fixed width keeps wrangler's name order numeric.
  - **Rejected:**
    - an `@migrations("…")` instance option, because options are creation
      parameters fixed for the resource's life, and a path isn't one;
    - a `bynk.toml` setting, since nobody has asked for a second layout and
      one can be added later without breaking anything;
    - `migrations/<Capability>/`, because two contexts may declare the same
      capability name.
- **B. Emission.** The build copies a database's files to
  `workers/<worker>/migrations/<database>/` and adds
  `migrations_dir = "migrations/<database>"` to its stanza, as `[assets]`
  copies its files. A database without a directory emits exactly what it did
  before. A named environment's `[env.<name>]` block copies the whole
  `d1_databases` array, and the key with it.
- **C. Deploy applies before the push, and records nothing.**
  - **The call:** `wrangler d1 migrations apply <binding> --remote` (plus
    `--env`), from the Worker's directory. The binding names the entry in every
    environment, where the database's name doesn't.
  - **When:** after the database is bound and the environment block written,
    before secrets and the push. It's a new phase in ADR 0194 D3's order.
  - **No prompt:** stdin is closed, so wrangler's `confirm` takes its "yes"
    fallback. Deploy's own confirmation and `--yes` already cover the apply.
  - **On failure:** wrangler rolls the failing file back and the earlier ones
    stay. Deploy stops before the push, so the previous code keeps serving.
  - **The ledger:** records nothing. The database's `d1_migrations` table
    records what ran (ADR 0194: nothing where another tool owns the state).
  - **The plan:** stays offline, so it lists each database's files without
    saying which are pending.
- **D. `bynk dev`** applies `--local` from each served Worker's directory
  before serving, forwarding a passthrough `--persist-to` so it reaches the
  database being served.
  - **Watching:** the watch fingerprint covers `migrations/`, and a rebuild
    applies again without a restart.
  - **Edits:** editing a file that was already applied is pointed out, because
    wrangler applies a file once, by name.
  - **Failure:** reported, and the session keeps serving.
  - **`--remote`:** nothing is applied, because only deploy changes a real
    database.
- **E. Tests start migrated.** The emitter embeds each database's files, in
  order, into both tiers' doubles. `makeIntegrationD1` runs each in its own
  transaction on every fresh database. A failure faults, naming the file.
- **F. Build checks:**
  - `bynk.d1.migration_name` (error): a misnamed or misplaced file, or a
    repeated number;
  - `bynk.d1.migration_unreadable` (error): a file that can't be read or isn't
    UTF-8;
  - `bynk.d1.migrations_unused` (warning): a directory no instance owns,
    usually a renamed capability.

  The SQL itself is wrangler's and SQLite's to check.
- **G. `D1` stays experimental, amending ADR 0662 H.** H gated graduation on
  this slice. Two language follow-ups still change what a `D1` call site
  looks like, decoding `Bool` from `0`/`1` and literal-only `sql`, so
  graduation waits for them.

**Consequences.**

- A schema evolves by adding files. No down migrations: neither D1 nor
  wrangler has them.
- For a moment during a deploy the old code serves against the new schema, so
  migrations should be additive: expand, deploy, and contract in a later one.
- A renamed capability is a new database whose directory must be renamed too.
  The unused-directory warning says so.
- The `d1_migrations` table exists in a deployed or `bynk dev` database, but
  not in a test double's.
