// Hyperdrive H1 (#3318, ADR 0799): the Postgres driver seam of
// `bynk.cloudflare`'s `Postgres` capability, over postgres.js (pinned exactly,
// a first-party npm dependency, ADR 0799 C).
//
// A Worker's compose passes `pgExecute` to `WorkersPostgres` as its third
// argument, so this module (and the package) enters a build only when a Worker
// has a `Postgres` instance. The binding itself never imports the driver.
//
// What this module owns, so that one place is tested against a fake `postgres`:
//
//   1. **`prepare: true` on every `sql.unsafe` call** (ADR 0799 A). In
//      postgres.js 3.4.5's `workerd` build `sql.unsafe()` defaults to
//      `prepare: false`, which makes it send type-discovery messages before each
//      query that Hyperdrive does not cache. The two calls below (a statement's,
//      and a transaction's `tx.unsafe`, #3320) are the only `unsafe` calls in the
//      runtime, and `postgres.test.ts` fails if any call reaches the driver
//      without the option.
//   2. **The connection**, opened from the binding's `connectionString` for one
//      statement and ended before the statement's outcome is returned. A
//      Worker may not carry an I/O object (a socket) from one request into
//      another, and an instance of a capability does not know where a request
//      ends, so nothing is cached. Hyperdrive pools the origin connections, so
//      this costs a local socket, not a database login.
//   3. **Parameters and rows** under ADR 0799 F: `int8` and `numeric` arrive as
//      strings, `timestamptz` as ISO-8601, `bytea` as base64, `json` and `jsonb`
//      nested.
//   4. **The failure** as a SQLSTATE class (ADR 0799 C / DECISION C of #3318).
//
// The connection string is a secret. It is passed to the driver and nowhere
// else: no message built here contains it, and a connection failure's message
// is a fixed string plus the driver's error *code*, never the driver's own text
// (which carries the host and port).
//
// The runtime is also copied into trees that never install the package, so the
// import's resolution is not type-checked, and the part of postgres.js this
// module uses is declared below (as `email.ts` does for `postal-mime`).
// @ts-ignore TS2307 when the package is not installed
import postgres from "postgres";

/** A `PgValue` as the binding hands it over (the tag and its one field). */
export type PgParam =
  | { readonly tag: "PgNull" }
  | { readonly tag: "PgInteger"; readonly value: number }
  | { readonly tag: "PgReal"; readonly value: number }
  | { readonly tag: "PgText"; readonly value: string }
  | { readonly tag: "PgBytea"; readonly value: Uint8Array }
  | { readonly tag: "PgBoolean"; readonly value: boolean }
  | { readonly tag: "PgTimestamp"; readonly value: number }
  | { readonly tag: "PgJson"; readonly value: string }
  | { readonly tag: "PgNumeric"; readonly value: string };

/** How a statement failed, before the binding gives it its `PgError` variant. */
export type PgFailure = {
  readonly kind: "constraint" | "serialization" | "connection" | "failed";
  /** The five-character SQLSTATE; empty when the driver reported none. */
  readonly sqlstate: string;
  readonly message: string;
};

export type PgOutcome =
  | { readonly ok: true; readonly rows: readonly Record<string, unknown>[]; readonly rowCount: number }
  | { readonly ok: false; readonly failure: PgFailure };

/** One statement of a `Postgres.transaction`, as the binding hands it over. */
export type PgStatementParam = { readonly sql: string; readonly params: readonly PgParam[] };

/** A transaction's outcome: every statement's row count, or the first failure (nothing applied). */
export type PgTxOutcome =
  | { readonly ok: true; readonly rowCounts: readonly number[] }
  | { readonly ok: false; readonly failure: PgFailure };

/**
 * Opens, runs one statement against, and closes a database. `transaction` (Hyperdrive
 * H3, #3320) runs a fixed list of statements in order on one connection, in one
 * database transaction: all apply, or none do.
 */
export interface PgExecute {
  (connectionString: string, sql: string, params: readonly PgParam[]): Promise<PgOutcome>;
  readonly transaction: (connectionString: string, statements: readonly PgStatementParam[]) => Promise<PgTxOutcome>;
}

/** The part of postgres.js's `Sql` this module uses. */
interface PgSql {
  unsafe(sql: string, params: readonly unknown[], options: { prepare: boolean }): PromiseLike<PgRows>;
  /** `sql.begin`: runs `fn` in a transaction, which commits when `fn` returns and rolls back when it throws. */
  begin<T>(fn: (tx: PgTx) => Promise<T>): Promise<T>;
  end(options?: { timeout?: number | null }): Promise<void>;
}
/** The transaction handle `sql.begin` passes: the same `unsafe`, on the transaction's connection. */
interface PgTx {
  unsafe(sql: string, params: readonly unknown[], options: { prepare: boolean }): PromiseLike<PgRows>;
}
interface PgRows extends ReadonlyArray<Record<string, unknown>> {
  readonly count?: number | null;
}
/**
 * How the driver reads and writes the date, time and JSON types (Hyperdrive H3,
 * #3320). postgres.js
 * parses `date`, `timestamp` and `timestamptz` alike with `new Date(text)`, which
 * would make a zone-less `timestamp` an instant (in the Worker's UTC) and a
 * `date` midnight, and an `infinity` an invalid date (which `jsonColumn` writes
 * as `null`). So only a finite `timestamptz` becomes a `Date` (and then ISO-8601,
 * ADR 0799 F); a `timestamp`, a `date` and an infinite value stay as the
 * server's text, which an `Instant` field refuses and a `String` field reads.
 */
export const PG_TYPES = (() => {
  // The driver's own `date` serializer, kept so a `Date` parameter binds as it did.
  const serialize = (x: unknown): string => (x instanceof Date ? x : new Date(x as string | number)).toISOString();
  return {
    // Correction to H1 (#3320): a `PgJson` is text that is already JSON, and the
    // driver's own `json` serializer is `JSON.stringify`, which registers for `json`
    // and `jsonb` alike and so quoted that text a second time (a jsonb string,
    // not the object or array). Text passes through unchanged; anything else (a
    // `PgText` bound where the server asks for json is text too) is the same.
    // Results are unchanged: `json` and `jsonb` columns arrive parsed and nested.
    json: {
      to: 114,
      from: [114, 3802],
      serialize: (x: unknown): string => (typeof x === "string" ? x : JSON.stringify(x)),
      parse: (text: string): unknown => JSON.parse(text),
    },
    // `date` and `timestamp`: the server's text, unparsed.
    date: { to: 1184, from: [1082, 1114], serialize, parse: (text: string): string => text },
    // `timestamptz`: a `Date`, unless it is `infinity` or `-infinity`.
    timestamptz: {
      to: 1184,
      from: [1184],
      serialize,
      parse: (text: string): Date | string => (/infinity/i.test(text) ? text : new Date(text)),
    },
  };
})();

/** What `postgres(connectionString, options)` is, for a fake to stand in. */
export type PgFactory = (connectionString: string, options: Record<string, unknown>) => PgSql;

/** The driver's connection codes: an error with one of these and no SQLSTATE is `connection`. */
const CONNECTION_CODES = new Set([
  "CONNECTION_CLOSED",
  "CONNECTION_DESTROYED",
  "CONNECT_TIMEOUT",
  "CONNECTION_ENDED",
  "ECONNREFUSED",
  "ECONNRESET",
  "ENOTFOUND",
  "ETIMEDOUT",
  "EPIPE",
  "EAI_AGAIN",
]);

/** The driver's fixed `code` of an error, or `""`. Only a string counts. */
function codeOf(e: unknown): string {
  const code = (e as { code?: unknown } | null | undefined)?.code;
  return typeof code === "string" ? code : "";
}

const SQLSTATE = /^[0-9A-Z]{5}$/;

/**
 * What `cloudflare:sockets` (through postgres.js' `workerd` build) and workerd's
 * Hyperdrive proxy throw when no connection can be made or one is lost: plain
 * errors with **no** `code`, such as `connection attempt failed` (an unreachable
 * port or an unresolvable host under local Hyperdrive) and `Network connection
 * lost.`. Matched on the message, and only for an error that is not the
 * database's own (a `PostgresError` has a SQLSTATE and is classified above).
 */
const SOCKET_FAILURE =
  /connection attempt failed|network connection lost|connection (?:was )?(?:refused|reset|closed|lost|timed out)|connect(?:ion)? (?:failed|timed out|timeout)|proxy request failed|could not connect|unable to connect|hostname (?:could not be|cannot be) resolved|getaddrinfo|socket (?:closed|hang up)/i;

/**
 * Map a driver error. A SQLSTATE of class `23` (integrity constraint) is
 * `constraint`; `40001` is `serialization`; a connection code with no SQLSTATE
 * is `connection`; everything else is `failed`. A connection failure's message
 * is fixed: the driver's own names the host and port.
 */
export function classifyPgError(e: unknown): PgFailure {
  const code = codeOf(e);
  const message = e instanceof Error ? e.message : String(e);
  if (SQLSTATE.test(code)) {
    if (code.startsWith("23")) return { kind: "constraint", sqlstate: code, message };
    if (code === "40001") return { kind: "serialization", sqlstate: code, message };
    return { kind: "failed", sqlstate: code, message };
  }
  if (CONNECTION_CODES.has(code)) {
    return { kind: "connection", sqlstate: "", message: `could not reach the database (${code})` };
  }
  // No SQLSTATE and no known code: a socket error from the runtime carries only a
  // message. The message returned is fixed, as above: the driver's own can name
  // the host and port.
  if (SOCKET_FAILURE.test(message)) {
    return { kind: "connection", sqlstate: "", message: "could not reach the database" };
  }
  return { kind: "failed", sqlstate: "", message };
}

/**
 * `text` with the connection string, and the password inside it, removed. The
 * driver does not put either in a message, and this is the second line: an
 * error built from a malformed string could.
 */
export function redactSecret(text: string, connectionString: string): string {
  let out = text;
  const secrets = [connectionString];
  try {
    const url = new URL(connectionString);
    secrets.push(url.password);
    try {
      secrets.push(decodeURIComponent(url.password));
    } catch {
      // A malformed escape: the raw form above is still removed.
    }
  } catch {
    // Not a URL: the whole string is all there is to remove.
  }
  for (const secret of secrets) {
    if (secret !== "") out = out.split(secret).join("[redacted]");
  }
  return out;
}

/** One parameter as the driver takes it. `Json` and `Numeric` go as text for the server to type. */
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

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

/**
 * A column value as JSON text carries it (ADR 0799 F): bytes as base64, a
 * `bigint` as a string, a date as ISO-8601, and everything else (a parsed
 * `json`/`jsonb` value, an array) nested as it is.
 */
export function jsonColumn(v: unknown): unknown {
  if (v instanceof Uint8Array) return base64(v);
  if (v instanceof ArrayBuffer) return base64(new Uint8Array(v));
  if (typeof v === "bigint") return v.toString();
  if (v instanceof Date) return Number.isNaN(v.getTime()) ? null : v.toISOString();
  if (Array.isArray(v)) return v.map(jsonColumn);
  if (v !== null && typeof v === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) out[k] = jsonColumn(x);
    return out;
  }
  return v;
}

/** A row ready for `JSON.stringify`. */
export function jsonRow(row: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(row)) out[k] = jsonColumn(v);
  return out;
}

/**
 * An executor over `factory` (postgres.js's default export, or a fake). One
 * client of one connection per statement (or per transaction), ended before the
 * outcome returns, whether it succeeded or not.
 */
export function makePgExecute(factory: PgFactory): PgExecute {
  // `max: 1`: one statement needs one connection. `fetch_types: false` is the
  // `workerd` build's default, written down so it is not left to it.
  const open = (connectionString: string): PgSql => factory(connectionString, { max: 1, fetch_types: false, types: PG_TYPES });
  const close = async (client: PgSql | undefined): Promise<void> => {
    if (client === undefined) return;
    try {
      await client.end();
    } catch {
      // The outcome stands; a failed close is not another one.
    }
  };
  const failed = (e: unknown, connectionString: string): { ok: false; failure: PgFailure } => {
    const failure = classifyPgError(e);
    return { ok: false, failure: { ...failure, message: redactSecret(failure.message, connectionString) } };
  };
  const countOf = (result: PgRows): number => (typeof result.count === "number" ? result.count : result.length);

  const execute = async (connectionString: string, sql: string, params: readonly PgParam[]): Promise<PgOutcome> => {
    let client: PgSql | undefined;
    try {
      client = open(connectionString);
      const result = await client.unsafe(sql, params.map(driverParam), { prepare: true });
      const rows = Array.from(result, jsonRow);
      return { ok: true, rows, rowCount: countOf(result) };
    } catch (e) {
      return failed(e, connectionString);
    } finally {
      await close(client);
    }
  };

  // Hyperdrive H3 (#3320): `sql.begin` holds one connection and one database
  // transaction. The statements run one after another, in order, each with
  // `{ prepare: true }` (ADR 0799 A); the first to fail throws, which rolls
  // back every earlier one and ends the transaction, so the failure is returned
  // and no later statement runs.
  const transaction = async (
    connectionString: string,
    statements: readonly PgStatementParam[],
  ): Promise<PgTxOutcome> => {
    let client: PgSql | undefined;
    try {
      client = open(connectionString);
      const rowCounts = await client.begin(async (tx) => {
        const counts: number[] = [];
        for (const statement of statements) {
          const result = await tx.unsafe(statement.sql, statement.params.map(driverParam), { prepare: true });
          counts.push(countOf(result));
        }
        return counts;
      });
      return { ok: true, rowCounts };
    } catch (e) {
      return failed(e, connectionString);
    } finally {
      await close(client);
    }
  };

  return Object.assign(execute, { transaction });
}

/**
 * Hyperdrive H4 (#3321): the opt-in test double's executors, by the connection
 * string its Hyperdrive binding carries. A multi-Worker test builds each
 * Worker's real compose, which hands `WorkersPostgres` this module's
 * `pgExecute`, so the double is reached by the string and no compose changes.
 * Only a string in the double's own scheme can be registered, and nothing but
 * `pglite.ts` registers one, so a deployed Worker's connection string never
 * finds an entry.
 */
const testDoubles = new Map<string, PgExecute>();

/** The scheme of a test double's connection string, which no database has. */
export const PG_TEST_DOUBLE_SCHEME = "bynk-pglite://";

export function registerPgTestDouble(connectionString: string, execute: PgExecute): void {
  if (!connectionString.startsWith(PG_TEST_DOUBLE_SCHEME)) {
    throw new Error("bynk: only a test double's connection string can be registered");
  }
  testDoubles.set(connectionString, execute);
}

const driverExecute = makePgExecute(postgres as unknown as PgFactory);

/** The executor a Worker's compose gives `WorkersPostgres`: postgres.js, or a registered test double. */
export const pgExecute: PgExecute = Object.assign(
  (connectionString: string, sql: string, params: readonly PgParam[]): Promise<PgOutcome> =>
    (testDoubles.get(connectionString) ?? driverExecute)(connectionString, sql, params),
  {
    transaction: (connectionString: string, statements: readonly PgStatementParam[]): Promise<PgTxOutcome> =>
      (testDoubles.get(connectionString) ?? driverExecute).transaction(connectionString, statements),
  },
);

// #1653: the `__` aliases emitted code imports.
export { pgExecute as __pgExecute };
export type { PgExecute as __PgExecute, PgOutcome as __PgOutcome, PgParam as __PgParam, PgStatementParam as __PgStatementParam, PgTxOutcome as __PgTxOutcome };
