---
level: patch
changelog: Design record for Hyperdrive (H0), covering the Postgres driver, the compatibility date and the surface; no surface ships yet
---

## ADR: hyperdrive-h0-driver-date-surface
title: Hyperdrive H0 settles the Postgres driver, the compatibility date and the surface before H1 builds it
summary: postgres.js pinned exactly; first-party npm dependencies need no approval row; the compatibility date moves past 2026-08-04 in a follow-up PR (H0b); `Postgres` and `MySql` kinds with their own value types; deploy reads the connection string from an environment variable

**Context.** Hyperdrive, the route to an existing Postgres or MySQL database, is
absent ([#3184](https://github.com/bynk-lang/bynk/issues/3184)). Two questions
block it and nothing else depends on them: which driver the binding wraps, and
how a Worker gets `nodejs_compat`. Proposal
[#3303](https://github.com/bynk-lang/bynk/issues/3303), decisions A–I. This
increment builds no surface; it fixes the ground so H1 is surface work only.

**Decision.**

- **A. Driver: postgres.js, pinned to an exact version; `pg` is the fallback.**
  Measured in a scratch directory with esbuild 0.25.0, bundling a minimal Worker
  that opens one connection from `env.HYPERDRIVE.connectionString` and runs one
  parameterised query, resolved with the `workerd` export condition:

  | | postgres.js 3.4.5 | `pg` 8.16.3 |
  | --- | --- | --- |
  | Bundle, unminified | 76.7 KB (20.2 KB gzip) | 177.5 KB (36.9 KB gzip) |
  | Bundle, minified | 41.0 KB (15.7 KB gzip) | 82.8 KB (25.9 KB gzip) |
  | Runtime dependencies | none | `pg-pool`, `pg-protocol`, `pg-types`, `pgpass`, `pg-connection-string`, `pg-cloudflare` |
  | Node built-ins left external | `node:buffer`, `node:events`, `node:stream` | `crypto`, `dns`, `events`, `fs`, `net`, `path`, `stream`, `string_decoder`, `tls`, `util` |
  | Socket | `cloudflare:sockets` through its `workerd` build (`cf/`) | `pg-cloudflare`, selected by its `workerd` export condition |

  postgres.js is about half the size, has no dependencies and needs fewer
  built-ins. **Both need `nodejs_compat`**: neither bundle runs without the
  `node:` modules above. These are bundle measurements and a read of each
  package's source. Nothing here ran against a live Hyperdrive or in `workerd`;
  H1's runtime smoke is where that is exercised.

  SQLSTATE is available on both. postgres.js throws a `PostgresError` that
  copies every server error field onto the error, so `code` is the SQLSTATE
  and `constraint_name` is present. `pg` throws a `DatabaseError` with `code`
  and `constraint`. A connection failure carries a string `code` on both
  (`CONNECTION_CLOSED` for postgres.js; `ECONNREFUSED`-style codes for `pg`)
  and no SQLSTATE, which is what `PgError.Connection` keys on.

  **One finding the proposal did not have.** Cloudflare's postgres.js guide says
  not to use `prepare: false`, because postgres.js then sends extra
  type-discovery messages before each query and Hyperdrive does not cache the
  statement. In postgres.js 3.4.5's `workerd` build, `sql.unsafe()` sets
  `prepare: false` by default (`cf/src/index.js`). A binding that takes its SQL
  as a literal string is on that path. The binding must therefore pass
  `{ prepare: true }` on every `sql.unsafe` call, and H1 carries a binding test that fails if any `sql.unsafe` call omits it
  (a fake `postgres` module asserting the options argument).
  `fetch_types: false` is already the default for the `workerd` build.

  Writing the wire protocol over `connect()` was rejected: it would be a
  Postgres client to maintain. `pg` was kept as the fallback, not the choice,
  for its size and its dependency tree.

- **B. `nodejs_compat`: raise `COMPATIBILITY_DATE` rather than add the flag per
  binding. The bump lands as its own PR (H0b), not in this one.** From
  `2026-08-04` Workers enable `nodejs_compat` and `nodejs_compat_v2` by
  default, so a date at or after it needs no flag, and a per-binding flag would
  be a second mechanism beside the date. This PR does not change
  `COMPATIBILITY_DATE`. Two sibling PRs (Secrets Store, #3301; outbound Email,
  #3302) add `wrangler.toml` fixtures, and a bump re-blesses every golden, so
  the bump follows them as H0b once both have merged.

  *What changes between `2026-07-01` and `2026-08-04`* (Cloudflare's
  compatibility-flags page, read 11 November 2026). Of the flags with a
  "default as of" date, exactly one falls in that window: **Node.js
  compatibility**, `2026-08-04`. Nothing else is switched on between the two
  dates. The nearest others are `2026-06-16` (before the current date) and
  `2026-09-08` (after the candidate). So for H0b:
  - `nodejs_compat` and `nodejs_compat_v2` are on by default. Before it,
    `nodejs_compat` also enabled v2 from `2024-09-23` to `2026-08-03`, so a
    Worker that opted in already ran v2; for one that did not, `node:` imports
    and the Node globals now resolve where they used to fail. Emitted Workers
    do not use the Node APIs today, and bundle size is unaffected unless code
    imports them.
  - A project that sets `nodejs_compat` in `[workers] compatibility_flags`
    keeps working. Cloudflare says the flags are redundant at the new date and
    are ignored, and existing projects need not remove them.
  - The opt-out is `no_nodejs_compat` plus `no_nodejs_compat_v2`.
  - `DEFAULT_COMPATIBILITY_FLAGS` (`global_fetch_strictly_public`) is not
    affected: the date does not turn it on.

  *Which date.* Part 3 of the release discipline picks the newest date that
  the smokes' `workerd` and the lag rule both allow. The candidate is
  **`2026-08-04`**, the first date at or after the window. It needs a wrangler
  whose `workerd` serves it: `4.120.1` (released 2026-08-10) is the first to
  carry `workerd` `1.20260804.1`; `4.119.0` and `4.120.0` carry `1.20260801.1`,
  which does not. On 11 November that is three months and a day back, so it
  just satisfies the lag rule. `WRANGLER_MIN` therefore moves from `4.107.0`
  to `4.120.1` in H0b, together with the doctor guide and the emission
  reference that `compat_date_policy.rs` pins. H0b re-checks the lag rule on
  the day it is cut; if it no longer holds, the review takes another date from
  the same page, and the change list above stands unless that page shows
  another flag in between.

  H0b is gated as Part 3 says: the full suite under `BYNK_REQUIRE_WORKERD=1`
  on the new date, a changelog entry, and an isolated golden re-bless commit.
  It needs no ADR of its own: the one flag that moves changes nothing a Bynk
  program can observe.

  **This moves the date out of the release cycle.** Part 3, step 1 reviews the
  date at each release. H0b is instead driven by a feature that needs a runtime
  default, and the ADR records that this is allowed: a feature that needs a
  newer runtime default may move the date between releases, under Part 3's gate
  (step 2) and record (step 3) unchanged. The next scheduled review then starts
  from the date H0b set, and need not find a bump the rule does not cover.

- **C. First-party npm dependencies: allowed, pinned to an exact version, no
  approval row. This amends ADR 0575**, which has every fetched adapter's npm
  `requires` approved and exempts only the project's own. *First-party* means
  a package that the toolchain itself emits into `out/package.json` for a
  built-in binding declared in `bynk-std`'s first-party sources; an adapter in
  any other package, fetched or not, is not first-party and stays under 0575.
  ADR 0575 approves fetched code; the first-party units ship
  inside the toolchain the user already trusts, and an upgrade is a reviewed
  toolchain change. The dependency still appears in `out/package.json` and in
  `bynk.lock`'s `[[npm]]` rows, marked first-party and never `approved`, so it
  is visible and a lock check still sees it. Exact pins (no range) mean a
  toolchain release, not an `npm install`, changes what runs. Each such
  binding is held to R-12 (`tsc --strict` over the binding on its own) and
  R-16 (a security review, as the connection string is a secret) from the
  retired standard-library track.

- **D. Two kinds, `Postgres` and `MySql`, both with binding prefix
  `HYPERDRIVE_`.** Placeholder style (`$1` against `?`), value set and error
  codes differ, and one `Hyperdrive` kind would need a dialect switch on every
  call. MySQL is the last slice (H5, `mysql2` ≥ 3.13.0 with `disableEval:
  true`).

- **E. A new `PgValue`, not `SqlValue`.** `SqlValue` is stable and closed and
  models SQLite's storage classes; adding `Timestamp` or `Json` would be a
  major-release change. `PgValue` is `Null | Integer | Real | Text | Bytea |
  Boolean | Timestamp(Instant) | Json(String) | Numeric(String)`, and
  `PgError` is an extensible sum (`Constraint`, `Serialization`, `Connection`,
  `Failed`), each carrying the SQLSTATE where there is one.

- **F. Rows are JSON text with fixed rules, read with `Json.decode`.** `int8`
  and `numeric` arrive as strings, so nothing past 2^53 is lost;
  `timestamptz` as ISO-8601; `bytea` as base64; `json` and `jsonb` nested.
  `decodeSql`'s SQLite rules (0 and 1 as flags) do not apply. Whether the codec
  reads an `Instant` field from ISO text is **not confirmed here**
  (`Instant.fromIso` exists; the codec's reading of an `Instant` field was
  not found in this survey), so H3 confirms it or adds it. On unifying
  `decodeSql` with the codec: **after H3.** The second consumer then exists, and
  the unification is made against two working decoders rather than one and a
  design.

- **G. Bynk does not own the schema.** No `migrations/` folder and no
  ADR 0763-style check by default. A later slice may check syntax and
  placeholder count with libpg_query when a `schema.sql` is supplied. The
  rule that SQL is a literal (ADR 0666 B) stays.

- **H. The connection string comes from an environment variable named in
  `bynk.toml`, never written to disk.** `[deploy.hyperdrive.Main]
  connection_env = "MAIN_DATABASE_URL"`. The deploy creates or updates the
  Hyperdrive config through the API and records its id in the ledger keyed by
  (environment, Worker, capability). The string appears nowhere in source, the
  ledger, `wrangler.toml` or logs. Adopting an existing config id per
  environment (#551) is the alternative path and a later option.

- **I. Tests: decided in H4, between stub-only and PGlite.** H1 ships with
  stub-only tests. PGlite (Postgres compiled to WASM) would be a
  contract-faithful double, as Node's SQLite is for D1, at the cost of a heavy
  dev dependency; that cost is weighed with H1–H3's experience.

**Slices (opened as sub-issues of #3184 by the coordinator).**

- **H0b:** the `COMPATIBILITY_DATE` bump to `2026-08-04`, `WRANGLER_MIN`, the
  doctor guide and emission reference, goldens re-blessed in one commit, after
  #3301 and #3302 merge.
- **H1:** `Postgres` `all`/`first`/`run` over postgres.js with `prepare: true`,
  experimental, stub-only tests, the reference page and the connect guide.
- **H2:** deploy provisioning and the ledger; `bynk dev` through
  `CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_<BINDING>`.
- **H3:** `select[T]` and `firstAs[T]` (ADR 0728's lowering with Postgres
  quoting), `transaction`, and the `Instant` codec check from F.
- **H4:** the test-double decision (I) and the optional schema-file check (G).
- **H5:** `MySql` over `mysql2`.

**Consequences.** No grammar, checker, emitter or runtime change, and no
Book change: nothing ships that a user can write. H1 is unblocked once H0b
lands (a Worker on a date before `2026-08-04` would need the flag instead).
The cost of C is that first-party npm code is trusted on the toolchain's
word, which the exact pin, R-12 and R-16 are there to keep honest. Six
simultaneous connections per invocation is Cloudflare's limit, so H1
documents it and calls out `par` over many queries. The date bump is a
runtime change for every deployed Worker, which is why it is its own PR.
