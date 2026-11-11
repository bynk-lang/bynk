---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: An experimental `Postgres` instance kind in `bynk.cloudflare` reaches an existing Postgres database through Hyperdrive over postgres.js, with `all`, `first` and `run`, typed `PgValue` parameters and `PgError` outcomes
---

## ADR: hyperdrive-h1-postgres-instance-kind
title: Hyperdrive H1: a `Postgres` instance kind over postgres.js
summary: Variant names, how the driver reaches the binding, one connection per statement, and the SQLSTATE mapping

**Context.** ADR 0799 settled the driver (postgres.js, pinned exactly), the value
and error types and the row rules, and ADR 0808 built the first-party npm
mechanism. Slice H1 ([#3318](https://github.com/bynk-lang/bynk/issues/3318),
part of #3184) builds the surface: `capability Main = Postgres`, then
`Main.all`, `first` and `run`. This record covers what H1 decides beyond those
two. Decisions A to C of #3318 (stub-only tests, the `prepare: true` guard, the
SQLSTATE mapping) were accepted as recommended and are carried out as written.

**Decision.**

- **A. Every new variant carries a `Pg` prefix.** A variant name shared by two
  enums of one unit resolves silently to one of them (the finding behind
  `SendFailed` and `DmarcPass`). Checked against `bynk.cloudflare`'s exports and
  the built-ins: `SqlValue` already has `Null`, `Integer`, `Real`, `Text` and
  `Boolean`; `SqlError` has `Constraint` and `Failed`; `Json` and `Timestamp` name
  built-in things, and `Connection` is the WebSocket type. So `PgValue` is
  `PgNull | PgInteger | PgReal | PgText | PgBytea | PgBoolean | PgTimestamp(Instant)
  | PgJson(String) | PgNumeric(String)` and `PgError` is `PgConstraint(sqlstate,
  message) | PgSerialization(message) | PgConnection(message) | PgFailed(sqlstate,
  message)`. All nine and all four are prefixed, not only the colliding ones, so
  a reader never has to know which are safe. `PgRun` is `{ rowCount: Int }`.
  `PgError` is extensible; `PgValue` and `PgRun` are stable records of an
  experimental capability.
- **B. The binding never imports the driver; a Worker's compose hands it in.**
  `WorkersPostgres(env, binding, execute)` takes the runtime's `pgExecute` as a
  third argument, which a Workers compose imports from the new `postgres`
  runtime module. The binding is one file shared by every project that consumes
  `bynk.cloudflare`, and a package enters a build when a runtime module that
  imports it is emitted (ADR 0808 A), so importing the module from the binding,
  even for a type, would put `postgres` in the `package.json` of every such
  project. The binding declares the structural types it needs itself. A bundle
  build passes no driver and the binding faults on use. `Reach` gains
  `CapabilityInstance("Postgres")`: the lock's first-party row is written when a
  context declares an instance, decided on the parsed declarations.
- **C. One connection per statement, ended before the statement answers.** The
  connection string comes from `env.<BINDING>.connectionString`. Each call opens
  a client with `max: 1` and `fetch_types: false`, runs the statement and calls
  `end()` in a `finally`, so a failed statement leaves nothing open. Cloudflare's
  guide opens one client per request and ends it with `ctx.waitUntil`; a
  capability instance knows neither the request nor `ctx`, and a Worker may not
  carry a socket from one request into another, so nothing can be cached across
  calls. Hyperdrive pools the origin connections, so the cost is a local socket,
  not a login. The price is that a `par` over many statements opens many
  connections, and Cloudflare allows six at once per invocation: the docs say so.
  H3's `transaction` needs one connection for several statements and will revisit
  this for that operation only.
- **D. The mapping and the message rules.** SQLSTATE class `23` is
  `PgConstraint`; `40001` is `PgSerialization`; an error with a driver connection
  code (`CONNECTION_CLOSED`, `CONNECT_TIMEOUT`, `ECONNREFUSED`, `ENOTFOUND` and
  the like) and no SQLSTATE is `PgConnection`; everything else, `40P01` and a
  non-SQLSTATE `code` included, is `PgFailed` carrying the SQLSTATE or an empty
  string. postgres.js builds a connection error's message from the host and port,
  so `PgConnection`'s message is fixed (`could not reach the database (<code>)`)
  and carries only the driver's code. Any message is also scrubbed of the
  connection string and of the password inside it. A server error's own message
  is kept (it names a constraint, not a value; `detail` is dropped).
- **E. `first` takes the first row; `run` reports the driver's count.** `first`
  does not rewrite the SQL to add a `LIMIT`: the statement is a literal and
  stays one. `rowCount` is postgres.js's `count` (rows inserted, updated or
  deleted, or returned by a `SELECT`), falling back to the row count.
- **F. The SQL is a literal; placeholders are `$n`; no schema check.** ADR 0666 B's
  literal rule is enforced for `Postgres` as for `D1`, with `$1`, `$2` in the
  note. A literal `Postgres` statement is not recorded as a site to prepare
  against the migrations: that schema is SQLite's. `$n` and the count of
  parameters are the server's to judge until H4's optional schema check.
- **G. Parameters.** `PgJson` and `PgNumeric` travel as text for the server to
  type from the column or cast; `PgTimestamp` is an epoch-millisecond `Instant`
  bound as a `timestamptz`; `PgBytea` is bytes.
- **H. Pin `postgres` 3.4.5, the version ADR 0799 A measured.** Later 3.x patches
  exist (3.4.9 is the newest); 4.x is days old. The finding that `sql.unsafe`
  defaults to `prepare: false` was made on 3.4.5, and the guard test, not the
  version, is what keeps it honest on an upgrade. Moving the pin is a reviewed
  toolchain change.
- **I. Tests.** Unit and integration: no double beyond a fault. Every operation
  of an instance faults, naming it, until the case stubs it, as `Service` does;
  an integration participant's `env.HYPERDRIVE_<NAME>` faults on its
  `connectionString`. The driver seam is tested in the runtime against a fake
  `postgres` factory: every `sql.unsafe` call must carry `{ prepare: true }`,
  parameters and rows follow ADR 0799 F, the SQLSTATE mapping is pinned, and the
  string never reaches a message. One test runs the real package against a
  refused local port, to check the wiring. **There is no workerd smoke**: it needs
  a real Postgres reachable from `wrangler dev`, which this build had none of,
  and is left to H2's `bynk dev` wiring.
- **J. Not in H1.** `transaction`, the typed `select` and `firstAs` (H3), deploy
  provisioning, the ledger and `bynk dev` (H2). `bynk deploy` does not create the
  Hyperdrive configuration: the emitted `[[hyperdrive]]` carries a placeholder
  `id`, and until H2 the configuration is made with `wrangler hyperdrive create`
  and the id written into `wrangler.toml`. `bynk deploy` does not yet replace the
  placeholder, so a deploy needs it set by hand.

**Security notes (R-16).** The connection string is a secret. It is read from the
binding and passed to the driver, and nowhere else: not logged, not in a fault
message (the missing-binding fault names the binding), not in any error. The
driver's connection error text names the host, so it is replaced by a fixed
string. The SQL is a literal and every value is a bound parameter. A statement is
always prepared (`prepare: true`), which is also what lets Hyperdrive cache it.
The remaining trust is the driver's: that it does not put the password in a
message, which the scrub backs up.

**Consequences.** A Worker with a `Postgres` instance gets `postgres` 3.4.5 in
`out/package.json` and a `first-party` `[[npm]]` row in `bynk.lock`; one without
does not. Every project that consumes `bynk.cloudflare` gains the `Pg*` types and
the `WorkersPostgres` binding, so each golden `cloudflare.ts` changes. `bynk dev`
and `bynk deploy` stop before wrangler, naming `postgres`, when it is not
installed in the build directory (ADR 0808).
