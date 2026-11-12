// Hyperdrive H4 (#3321, ADR 0799 I): the opt-in `Postgres` test double.
//
// `[test] postgres = "pglite"` in `bynk.toml` makes a `Postgres` instance in a
// test run its statements on PGlite (Postgres compiled to WASM, a first-party
// npm dependency pinned exactly, ADR 0799 C), as `D1` runs on Node's SQLite.
// Without the opt-in a test never reaches a database and every operation
// faults until it is stubbed. PGlite is installed only for a project that opts in:
// this module is the only one that imports it, and a build emits the module
// only when the harness references it.
//
// What this module owns:
//
//   1. **A fresh database per case, at the cost of one start-up per run.** PGlite
//      takes about 1.5 seconds to start, which is too much to pay per case, so a
//      database is started once per instance and *reset* for each case (its
//      schema dropped and re-made, a few milliseconds), then the project's
//      `schema.sql` is applied. A case never sees another's rows, sequences or
//      tables. PGlite has one session, so a database is used by one case at a time,
//      which is how cases run.
//   2. **The contract of ADR 0799 F, not PGlite's.** `int8` and `numeric` as
//      strings, `timestamptz` as ISO-8601, `bytea` as base64, `json` and `jsonb`
//      nested, and a `date` or zone-less `timestamp` as the server's text, the
//      same as `postgres.ts` reads them from postgres.js.
//   3. **The failure** through `classifyPgError`: PGlite throws the server's own
//      error with its SQLSTATE, so a constraint violation is `23505` here as it
//      is on Postgres.
//
// What it cannot be: a second connection. PGlite is one session, so a
// serialisation failure (`40001`) cannot arise (it needs two concurrent
// transactions) and a case that needs one forces it with a stub.
//
// The runtime is also copied into trees that never install the package, so the
// import's resolution is not type-checked, and the part of PGlite used here is
// declared below.
// @ts-ignore TS2307 when the package is not installed
import { PGlite } from "@electric-sql/pglite";
import { PG_TEST_DOUBLE_SCHEME, classifyPgError, jsonRow, registerPgTestDouble, type PgExecute, type PgOutcome, type PgParam, type PgStatementParam, type PgTxOutcome } from "./postgres.ts";

/** The part of PGlite's query result this module reads. */
interface PgliteResult {
  readonly rows: readonly Record<string, unknown>[];
  readonly affectedRows?: number;
}
interface PgliteQueryOptions {
  readonly parsers: Record<number, (text: string) => unknown>;
  readonly serializers: Record<number, (value: unknown) => string>;
}
interface PgliteHandle {
  query(sql: string, params: readonly unknown[], options: PgliteQueryOptions): Promise<PgliteResult>;
}
/** The part of a PGlite database this module uses. */
interface PgliteDb extends PgliteHandle {
  readonly waitReady: Promise<void>;
  exec(sql: string): Promise<unknown>;
  close(): Promise<void>;
  transaction<T>(fn: (tx: PgliteHandle) => Promise<T>): Promise<T>;
}

/** What `new PGlite()` is, for a fake to stand in. */
export type PgliteFactory = () => PgliteDb;

/**
 * How PGlite reads and writes the date, time and JSON types: the rules of
 * `PG_TYPES` in `postgres.ts`, by type OID. Only a finite `timestamptz` becomes
 * a `Date` (then ISO-8601); `int8` and `numeric` are text; a `date`, a `timestamp` and an infinite value stay as
 * the server's text; `json` and `jsonb` are parsed, and text bound to them is
 * already JSON and is not quoted a second time.
 */
export const PGLITE_OPTIONS: PgliteQueryOptions = {
  parsers: {
    // `int8` and `numeric` are text whatever their size (ADR 0799 F); PGlite
    // would otherwise hand a small `int8` over as a number.
    20: (text) => text,
    1700: (text) => text,
    114: (text) => JSON.parse(text),
    3802: (text) => JSON.parse(text),
    1082: (text) => text,
    1114: (text) => text,
    1184: (text) => (/infinity/i.test(text) ? text : new Date(text)),
  },
  serializers: {
    114: (v) => (typeof v === "string" ? v : JSON.stringify(v)),
    3802: (v) => (typeof v === "string" ? v : JSON.stringify(v)),
  },
};

/** One parameter as PGlite takes it, the same mapping as `postgres.ts`'s. */
function driverParam(p: PgParam): unknown {
  switch (p.tag) {
    case "PgNull":
      return null;
    case "PgTimestamp":
      return new Date(p.value);
    default:
      return p.value;
  }
}

function countOf(result: PgliteResult): number {
  return Math.max(typeof result.affectedRows === "number" ? result.affectedRows : 0, result.rows.length);
}

/** Databases started in this process, one per instance, reused by every case. */
const started = new Map<string, Promise<PgliteDb>>();

/** `factory`'s database for `key`, started once. */
function database(key: string, factory: PgliteFactory): Promise<PgliteDb> {
  let db = started.get(key);
  if (db === undefined) {
    db = (async () => {
      const made = factory();
      await made.waitReady;
      return made;
    })();
    started.set(key, db);
  }
  return db;
}

/**
 * Closes every database this process started. PGlite (Postgres' own interval
 * timer, in WebAssembly) leaves a ten-second timer running after its last
 * statement, which holds a Node process open that long; `close()` clears it. The
 * test runner calls this when the run is over, through `globalThis`, as it
 * needs no import of the module.
 */
export async function closePgliteDatabases(): Promise<void> {
  const open = [...started.values()];
  started.clear();
  for (const pending of open) {
    try {
      await (await pending).close();
    } catch {
      // A database that never started, or already closed, has nothing to clear.
    }
  }
}
(globalThis as unknown as { __bynkPgliteTeardown?: () => Promise<void> }).__bynkPgliteTeardown = closePgliteDatabases;

/**
 * A fresh database for one case: the schema `public` dropped and made again, then
 * `schemaSql` applied. A `schemaSql` that fails faults, naming the failure, as a
 * D1 migration does, rather than letting every statement of the case echo it.
 */
async function reset(db: PgliteDb, schemaSql: string): Promise<void> {
  await db.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;");
  if (schemaSql.trim() === "") return;
  try {
    await db.exec(schemaSql);
  } catch (e) {
    throw new Error(`bynk: the Postgres test double's \`schema.sql\` failed: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/** An executor over one reset database. */
export function makePgliteExecute(ready: Promise<PgliteDb>): PgExecute {
  const execute = async (_connectionString: string, sql: string, params: readonly PgParam[]): Promise<PgOutcome> => {
    try {
      const db = await ready;
      const result = await db.query(sql, params.map(driverParam), PGLITE_OPTIONS);
      return { ok: true, rows: result.rows.map(jsonRow), rowCount: countOf(result) };
    } catch (e) {
      if (e instanceof Error && e.message.startsWith("bynk: ")) throw e;
      return { ok: false, failure: classifyPgError(e) };
    }
  };
  const transaction = async (
    _connectionString: string,
    statements: readonly PgStatementParam[],
  ): Promise<PgTxOutcome> => {
    try {
      const db = await ready;
      const rowCounts = await db.transaction(async (tx) => {
        const counts: number[] = [];
        for (const statement of statements) {
          counts.push(countOf(await tx.query(statement.sql, statement.params.map(driverParam), PGLITE_OPTIONS)));
        }
        return counts;
      });
      return { ok: true, rowCounts };
    } catch (e) {
      if (e instanceof Error && e.message.startsWith("bynk: ")) throw e;
      return { ok: false, failure: classifyPgError(e) };
    }
  };
  return Object.assign(execute, { transaction });
}

/**
 * The `Postgres` double of one instance, for a case: the `env` that
 * `WorkersPostgres` reads its Hyperdrive binding from, and the executor it runs
 * statements with. The database is reset when this is called (once per case), and
 * the first statement waits for it.
 *
 * `factory` is PGlite's constructor, or a fake. Its default loads the package on
 * first use.
 */
export function makeIntegrationPostgres(
  binding: string,
  schemaSql: string = "",
  factory: PgliteFactory = () => new PGlite() as unknown as PgliteDb,
): { readonly env: Record<string, { readonly connectionString: string }>; readonly execute: PgExecute } {
  const ready = database(binding, factory).then(async (db) => {
    await reset(db, schemaSql);
    return db;
  });
  // A reset that fails is reported by the first statement; without a handler
  // here an instance whose case never runs a statement would leave it unhandled.
  ready.catch(() => undefined);
  return {
    env: { [binding]: { connectionString: `${PG_TEST_DOUBLE_SCHEME}${binding}` } },
    execute: makePgliteExecute(ready),
  };
}

/**
 * The Hyperdrive binding of a multi-Worker test participant, keyed by the
 * participant and its binding, backed by a database reset for the case. Its
 * connection string selects the double in `postgres.ts`'s `pgExecute`, which the
 * participant's compose already uses.
 */
export function makeIntegrationPgliteBinding(
  key: string,
  schemaSql: string = "",
  factory?: PgliteFactory,
): { readonly connectionString: string } {
  const { env, execute } = makeIntegrationPostgres(key, schemaSql, factory);
  const hyperdrive = env[key] as { readonly connectionString: string };
  registerPgTestDouble(hyperdrive.connectionString, execute);
  return hyperdrive;
}

// #1653: the `__` aliases emitted code imports.
export { makeIntegrationPgliteBinding as __makeIntegrationPgliteBinding, makeIntegrationPostgres as __makeIntegrationPostgres };
