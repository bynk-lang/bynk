# 0662 — D1 databases as derived capabilities with two-step rows and errors as values

- **Status:** Accepted (v0.448)

**Context.** A Bynk Worker had no relational store: agents hold per-key state,
KV and R2 hold values by key. The standard-library track lists D1 as wanted
(§6.3) and settles how structured results cross the boundary (§8 D4): an
operation returns the whole result as one JSON `String` that the caller decodes
with `Json.decode[T]`, and SQL parameters are a first-party closed sum
(`SqlValue`), since a one-step `query[T]` needs a decoder synthesised at the
call site. D4 notes two gaps D1's slice must handle: SQLite has no booleans
(a flag column is `0` or `1`) and blob columns, neither of which JSON
represents. Track A12 gives the database's physical name to `bynk deploy` and
its minted id to the ledger, as KV already works. The binding is
`env.X.prepare(sql).bind(...values)` with `all`, `first` and `run`, plus
`env.X.batch(statements)`, a transaction. No first-party service needs D1
today: the registry stores in agents and R2.

**Decision.**

- **A. `D1` is instance-only and takes no options.** An `Instantiable` row
  (prefix `D1`, `options: &[]`, `bare: false`): `capability Orders = D1` binds
  as `D1_ORDERS`. A bare use is `bynk.capability.instance_only`; any option is
  `bynk.capability.unknown_option`.
- **B. Four operations, named after D1's own.** `all(sql, params) ->
  Result[String, SqlError]` (every row, a JSON array of objects keyed by column
  name), `first(sql, params) -> Result[Option[String], SqlError]`, `run(sql,
  params) -> Result[SqlRun, SqlError]` (`changes`, `lastRowId`) and
  `batch(List[SqlStatement]) -> Result[List[SqlRun], SqlError]`, one
  transaction. Parameters are positional (`?1`, `?2`, …), always bound, never
  spliced.
- **C. `SqlValue` is named by SQLite storage class.** `Null`, `Integer(Int)`,
  `Real(Float)`, `Text(String)`, `Blob(Bytes)` and `Boolean(Bool)`, which binds
  as `1` or `0`. *Changed from the proposal*, which spelt the variants `Int`,
  `Float` and `Bool`: as variant names those read as the built-in types they
  wrap, and the storage-class names say what SQLite stores.
- **D. Errors are values.** Every operation returns a `Result`. A failure whose
  message carries `SQLITE_CONSTRAINT` is `Constraint(message)`, anything else
  `Failed(message)`. `SqlError` is extensible. *Why:* a UNIQUE violation is an
  outcome a handler answers (a `409`), unlike a missing KV key, so faulting
  would force every insert through a pre-check that races. A missing binding
  still faults: it is a deployment mistake, not an outcome.
- **E. Blobs become base64 in the binding; booleans are left to the caller.**
  The binding replaces each blob value (an `ArrayBuffer`, a typed-array view or
  a byte array, since runtimes differ) with its standard base64, which the
  `Bytes` codec reads. It cannot do the same for booleans: it does not know
  which integer column is a flag. A flag field is declared `Int`; a `Bool`
  field fails the decode loudly. An integer past ±2^53 likewise fails the
  decode rather than rounding.
- **F. Provisioning through the ledger.** The database is named
  `<worker>-<capability, kebab>`, environment-qualified. `bynk deploy` runs
  `wrangler d1 create`, reads the minted `database_id` from its output, records
  name and id (`d1_instances`) before the push, and writes both into the
  config; a database of that name already on the account is adopted, its id
  read from `wrangler d1 list --json`. A removed instance's database is
  reported as an orphan and never deleted. `bynk dev -- --remote` binds the
  recorded database.
  - **Rejected:** letting wrangler auto-provision by omitting the id. What it
    mints is then recorded where the ledger cannot see it, losing the orphan
    report and per-environment ids (ADR 0194).
- **G. Tests run real SQL.** Both test tiers bind an in-memory SQLite database
  through Node's built-in `node:sqlite` (Node 22.13 or later); the unit-tier
  double is the real `WorkersD1` over it, so a test exercises the same row and
  error mapping as production. The double binds an integral number as
  `INTEGER`, enforces foreign keys and words a failure as D1 does. Node 22.13
  is the first 22.x to load `node:sqlite` without a flag, so the Node floor
  rises to it.
- **H. Experimental,** until schema migrations (P4b) land.

**Consequences.**

- Every query site writes its own `Json.decode`, as D4 accepted.
- Routed out, as language work under ADR 0023: decoding `Bool` from `0`/`1`
  (a codec change), and literal-only `sql` arguments, which would rule out SQL
  injection by construction as K6c did for regex patterns.
- A new database is empty until P4b's migrations: the application creates its
  tables, or the developer runs `wrangler d1 execute`.
- The Node floor rises from 22 to 22.13 (`NODE_MINOR_FLOOR`), so `bynk
  doctor` warns below it and CI's floor job runs exactly 22.13.0. Only the D1
  double needs it, but a floor a test tier silently exceeds is not a floor.
