// Hyperdrive H5 (#3322, ADR 0799 D): the MySQL driver seam of
// `bynk.cloudflare`'s `MySql` capability, over mysql2 (pinned exactly, a
// first-party npm dependency, ADR 0799 C).
//
// A Worker's compose passes `mysqlExecute` to `WorkersMySql` as its third
// argument, so this module (and the package) enters a build only when a Worker
// has a `MySql` instance. The binding itself never imports the driver.
//
// What this module owns, so that one place is tested against a fake `mysql2`:
//
//   1. **`disableEval: true` on every connection.** Workers forbid `eval` and
//      `new Function`, which mysql2 otherwise uses to build its row parsers;
//      Cloudflare's guide calls the option "required to enable mysql2
//      compatibility for Workers", and the minimum version is 3.13.0.
//      `mysql.test.ts` fails if a connection is opened without it.
//   2. **`query`, not `execute`.** `execute` prepares a server-side statement,
//      which does not survive Hyperdrive's transaction pooling (the next
//      statement may run on another origin connection), and mysql2 cannot bind
//      `LIMIT ?, ?` through it. `query` has the driver substitute each `?` with
//      an escaped literal before the statement is sent, so a parameter is never
//      spliced by Bynk and never reaches the server as SQL text.
//   3. **The connection**, opened from the binding's endpoint for one statement
//      (or one transaction) and ended before the outcome is returned: a Worker
//      may not carry an I/O object from one request into another. Hyperdrive
//      pools the origin connections, so this costs a local socket.
//   4. **Parameters and rows**: an unsigned integer and a `DECIMAL` bind as
//      text and arrive as text, so nothing past 2^53 is lost; every date and
//      time column arrives as the server's text, with no zone put on it; a
//      `TINYINT(1)` arrives as a flag; `BLOB` columns as base64; `JSON` nested.
//   5. **The failure**, keyed on the MySQL error number.
//
// The connection's password is a secret. It is passed to the driver and nowhere
// else: no message built here contains it, and a connection failure's message
// is a fixed string plus the driver's error *code*, never the driver's own text
// (which names the host and port).
//
// The runtime is also copied into trees that never install the package, so the
// import's resolution is not type-checked, and the part of mysql2 this module
// uses is declared below (as `postgres.ts` does for postgres.js).
// @ts-ignore TS2307 when the package is not installed
import { createConnection as mysql2CreateConnection } from "mysql2/promise";

/** A `MySqlValue` as the binding hands it over (the tag and its one field). */
export type MySqlParam =
  | { readonly tag: "MySqlNull" }
  | { readonly tag: "MySqlInteger"; readonly value: number }
  | { readonly tag: "MySqlUnsigned"; readonly value: string }
  | { readonly tag: "MySqlReal"; readonly value: number }
  | { readonly tag: "MySqlText"; readonly value: string }
  | { readonly tag: "MySqlBytes"; readonly value: Uint8Array }
  | { readonly tag: "MySqlBoolean"; readonly value: boolean }
  | { readonly tag: "MySqlTimestamp"; readonly value: number }
  | { readonly tag: "MySqlDateTime"; readonly value: string }
  | { readonly tag: "MySqlJson"; readonly value: string }
  | { readonly tag: "MySqlDecimal"; readonly value: string };

/** Where a Hyperdrive binding says the database is. `password` is a secret. */
export interface MySqlEndpoint {
  readonly host: string;
  readonly port: number;
  readonly user: string;
  readonly password: string;
  readonly database: string;
}

/** How a statement failed, before the binding gives it its `MySqlError` variant. */
export type MySqlFailure = {
  readonly kind: "constraint" | "serialization" | "connection" | "failed";
  /** The MySQL error number; `0` when the driver reported none. */
  readonly code: number;
  readonly message: string;
};

export type MySqlOutcome =
  | {
      readonly ok: true;
      readonly rows: readonly Record<string, unknown>[];
      readonly rowCount: number;
      /** `AUTO_INCREMENT` id of the row a write inserted; `0` when none. */
      readonly insertId: number;
    }
  | { readonly ok: false; readonly failure: MySqlFailure };

/** One statement of a `MySql.transaction`, as the binding hands it over. */
export type MySqlStatementParam = { readonly sql: string; readonly params: readonly MySqlParam[] };

/** A transaction's outcome: every statement's counts, or the first failure (nothing applied). */
export type MySqlTxOutcome =
  | {
      readonly ok: true;
      readonly runs: readonly { readonly rowCount: number; readonly insertId: number }[];
    }
  | { readonly ok: false; readonly failure: MySqlFailure };

/** Opens, runs one statement against, and closes a database; `transaction` runs a fixed list in one. */
export interface MySqlExecute {
  (endpoint: MySqlEndpoint, sql: string, params: readonly MySqlParam[]): Promise<MySqlOutcome>;
  readonly transaction: (
    endpoint: MySqlEndpoint,
    statements: readonly MySqlStatementParam[],
  ) => Promise<MySqlTxOutcome>;
}

/** The part of a mysql2 field description `typeCast` reads. */
interface MySqlField {
  readonly type: string;
  readonly length: number;
  string(): string | null;
}

/** The part of a mysql2 promise `Connection` this module uses. */
interface MySqlConnection {
  query(sql: string, values: readonly unknown[]): Promise<[unknown, unknown]>;
  beginTransaction(): Promise<void>;
  commit(): Promise<void>;
  rollback(): Promise<void>;
  end(): Promise<void>;
}

/** What `mysql2/promise`'s `createConnection(options)` is, for a fake to stand in. */
export type MySqlFactory = (options: Record<string, unknown>) => Promise<MySqlConnection>;

/**
 * How the driver reads a column (ADR 0799 F's rules, as MySQL has them). Only
 * three types are cast by hand:
 *
 * - `TINYINT(1)`, MySQL's flag, becomes `true` or `false`, as a Postgres
 *   `boolean` does. Any other `TINYINT` width stays a number.
 * - `DATETIME`, `TIMESTAMP` and `DATE` stay the server's text
 *   (`2026-11-12 09:30:00`), which carries no zone: `DATETIME` has none, and a
 *   `TIMESTAMP` is shown in the *session's* zone, which Hyperdrive's pooling
 *   does not let a Worker pin. Putting `Z` on either would invent an instant,
 *   so an `Instant` field refuses the text and a `String` field reads it.
 * - everything else is the driver's default, with `supportBigNumbers` and
 *   `bigNumberStrings` making `BIGINT` (signed or unsigned) and `DECIMAL` text.
 */
export function mysqlTypeCast(field: MySqlField, next: () => unknown): unknown {
  if (field.type === "TINY" && field.length === 1) {
    const text = field.string();
    return text === null ? null : text !== "0";
  }
  if (field.type === "DATETIME" || field.type === "TIMESTAMP" || field.type === "DATE") {
    return field.string();
  }
  return next();
}

/** Connection errors the driver reports by `code`, with no MySQL error number. */
const CONNECTION_CODES = new Set([
  "PROTOCOL_CONNECTION_LOST",
  "ECONNREFUSED",
  "ECONNRESET",
  "ENOTFOUND",
  "ETIMEDOUT",
  "EPIPE",
  "EAI_AGAIN",
  "ER_CON_COUNT_ERROR",
]);

/** MySQL error numbers that are integrity-constraint violations: duplicate key, not-null, foreign key (child and parent), check. */
const CONSTRAINT_ERRNOS = new Set([1062, 1048, 1452, 1451, 3819]);

/** Deadlock and lock-wait timeout: the statement may succeed if tried again. */
const RETRY_ERRNOS = new Set([1213, 1205]);

/**
 * Access and authentication failures: `1044` (access denied to a database),
 * `1045` (access denied for a user), `1049` (unknown database) and `1698`
 * (socket-style authentication refused). The server's text names the user and
 * the client's host (`Access denied for user 'app'@'10.0.0.5'`), which a
 * handler must not receive, so these are `connection` failures with a fixed
 * message and the error number.
 */
const ACCESS_ERRNOS = new Set([1044, 1045, 1049, 1698]);

/** Server-side "connection gone" numbers (`2002`-`2013` are the client library's). */
const CONNECTION_ERRNOS = new Set([1040, 1053, 1129, 1130, 1152, 1153, 1154, 1155, 1156, 1157, 1158, 1159, 1160, 1161]);

/**
 * What workerd's Hyperdrive proxy throws when no connection can be made: plain
 * errors with no `code` and no number. Matched on the message, and only for an
 * error that is not the database's own (which has an `errno`).
 */
const SOCKET_FAILURE =
  /connection attempt failed|network connection lost|connection (?:was )?(?:refused|reset|closed|lost|timed out)|connect(?:ion)? (?:failed|timed out|timeout)|proxy request failed|could not connect|unable to connect|hostname (?:could not be|cannot be) resolved|getaddrinfo|socket (?:closed|hang up)|can't add new command when connection is in closed state/i;

function errnoOf(e: unknown): number {
  const errno = (e as { errno?: unknown } | null | undefined)?.errno;
  return typeof errno === "number" && Number.isInteger(errno) ? errno : 0;
}

function codeOf(e: unknown): string {
  const code = (e as { code?: unknown } | null | undefined)?.code;
  return typeof code === "string" ? code : "";
}

/**
 * Map a driver error on its MySQL error number. `1062` (duplicate key), `1048`
 * (column cannot be null), `1452` (foreign key, child), `1451` (foreign key,
 * parent) and `3819` (check constraint) are `constraint`; `1213` (deadlock) and
 * `1205` (lock wait timeout) are `serialization`; a connection code, a
 * connection-class number or a socket failure is `connection`; everything else
 * is `failed`. A connection failure's message is fixed: the driver's own names
 * the host and port.
 */
export function classifyMySqlError(e: unknown): MySqlFailure {
  const errno = errnoOf(e);
  const code = codeOf(e);
  const message = e instanceof Error ? e.message : String(e);
  if (CONSTRAINT_ERRNOS.has(errno)) return { kind: "constraint", code: errno, message };
  if (RETRY_ERRNOS.has(errno)) return { kind: "serialization", code: errno, message };
  if (ACCESS_ERRNOS.has(errno)) {
    return { kind: "connection", code: errno, message: `could not connect to the database (access refused, ${errno})` };
  }
  if (CONNECTION_CODES.has(code) || CONNECTION_ERRNOS.has(errno) || (errno >= 2002 && errno <= 2013)) {
    return {
      kind: "connection",
      code: errno,
      message: `could not reach the database (${code === "" ? String(errno) : code})`,
    };
  }
  if (errno === 0 && SOCKET_FAILURE.test(message)) {
    return { kind: "connection", code: 0, message: "could not reach the database" };
  }
  return { kind: "failed", code: errno, message };
}

/**
 * `text` with the endpoint's password, user and host, and any `'user'@'host'`
 * account, removed: the second line of defence behind the fixed messages above.
 */
export function redactEndpoint(text: string, endpoint: MySqlEndpoint): string {
  // MySQL writes an account as `'user'@'host'`; whatever those are, they go.
  let out = text.replace(/'[^']*'@'[^']*'/g, "[redacted]");
  const secrets = [endpoint.password, encodeURIComponent(endpoint.password), endpoint.user, endpoint.host];
  for (const secret of secrets) {
    // A secret of fewer than three characters is left: removing it would mangle the message.
    if (secret.length >= 3) out = out.split(secret).join("[redacted]");
  }
  return out;
}

/** `YYYY-MM-DD HH:MM:SS.mmm` in UTC: what a `DATETIME(3)` or a UTC `TIMESTAMP(3)` takes. */
export function utcDateTimeText(epochMillis: number): string {
  return new Date(epochMillis).toISOString().replace("T", " ").replace("Z", "");
}

/** One parameter as the driver takes it. Unsigned, decimal, JSON and date text go as text for the server to type. */
function driverParam(p: MySqlParam): unknown {
  switch (p.tag) {
    case "MySqlNull":
      return null;
    case "MySqlBoolean":
      return p.value ? 1 : 0;
    case "MySqlTimestamp":
      return utcDateTimeText(p.value);
    case "MySqlBytes":
      // A Node `Buffer` is what the driver escapes as a hex literal.
      return (globalThis as { Buffer?: { from(b: Uint8Array): unknown } }).Buffer?.from(p.value) ?? p.value;
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
 * A column value as JSON text carries it: bytes as base64, a `bigint` as a
 * string, and everything else (a parsed `JSON` column, text, a number, a flag)
 * nested as it is.
 */
export function mysqlJsonColumn(v: unknown): unknown {
  if (v instanceof Uint8Array) return base64(v);
  if (v instanceof ArrayBuffer) return base64(new Uint8Array(v));
  if (typeof v === "bigint") return v.toString();
  if (v instanceof Date) return Number.isNaN(v.getTime()) ? null : v.toISOString();
  if (Array.isArray(v)) return v.map(mysqlJsonColumn);
  if (v !== null && typeof v === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) out[k] = mysqlJsonColumn(x);
    return out;
  }
  return v;
}

/** A row ready for `JSON.stringify`. */
export function mysqlJsonRow(row: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(row)) out[k] = mysqlJsonColumn(v);
  return out;
}

/** A write's header, as the driver answers one. */
interface ResultHeader {
  readonly affectedRows?: unknown;
  readonly insertId?: unknown;
}

/**
 * A header field as an `Int`. A missing or non-numeric field (which the driver
 * never produces) reads as `0`; an integer past the safe-integer domain throws,
 * so it surfaces as `MySqlFailed` rather than as `0`, which means "inserted none".
 */
function numberOf(v: unknown, field: string): number {
  const n = typeof v === "string" && v.trim() !== "" ? Number(v) : v;
  if (typeof n !== "number" || !Number.isInteger(n)) return 0;
  if (!Number.isSafeInteger(n)) {
    throw new Error(`${field} past the Int range: ${String(v)}`);
  }
  return n;
}

/** The connection options (`disableEval` is first, and not optional: ADR 0799 D, Cloudflare's mysql2 guide). */
export function connectionOptions(endpoint: MySqlEndpoint): Record<string, unknown> {
  return {
    host: endpoint.host,
    port: endpoint.port,
    user: endpoint.user,
    password: endpoint.password,
    database: endpoint.database,
    disableEval: true,
    supportBigNumbers: true,
    bigNumberStrings: true,
    decimalNumbers: false,
    dateStrings: true,
    multipleStatements: false,
    typeCast: mysqlTypeCast,
  };
}

/**
 * An executor over `factory` (mysql2/promise's `createConnection`, or a fake).
 * One connection per statement (or per transaction), ended before the outcome
 * returns, whether it succeeded or not.
 */
export function makeMySqlExecute(factory: MySqlFactory): MySqlExecute {
  const close = async (conn: MySqlConnection | undefined): Promise<void> => {
    if (conn === undefined) return;
    try {
      await conn.end();
    } catch {
      // The outcome stands; a failed close is not another one.
    }
  };
  const failed = (e: unknown, endpoint: MySqlEndpoint): { ok: false; failure: MySqlFailure } => {
    const failure = classifyMySqlError(e);
    return { ok: false, failure: { ...failure, message: redactEndpoint(failure.message, endpoint) } };
  };
  // A `SELECT` answers its rows; a write answers a header. Either way, the count of rows touched.
  const read = (
    answer: unknown,
  ): { rows: readonly Record<string, unknown>[]; rowCount: number; insertId: number } => {
    if (Array.isArray(answer)) {
      const rows = (answer as Record<string, unknown>[]).map(mysqlJsonRow);
      return { rows, rowCount: rows.length, insertId: 0 };
    }
    const header = (answer ?? {}) as ResultHeader;
    return { rows: [], rowCount: numberOf(header.affectedRows, "affectedRows"),
      insertId: numberOf(header.insertId, "insertId"), };
  };
  const query = async (conn: MySqlConnection, sql: string, params: readonly MySqlParam[]) => {
    const [answer] = await conn.query(sql, params.map(driverParam));
    return read(answer);
  };

  const execute = async (endpoint: MySqlEndpoint, sql: string, params: readonly MySqlParam[]): Promise<MySqlOutcome> => {
    let conn: MySqlConnection | undefined;
    try {
      conn = await factory(connectionOptions(endpoint));
      return { ok: true, ...(await query(conn, sql, params)) };
    } catch (e) {
      return failed(e, endpoint);
    } finally {
      await close(conn);
    }
  };

  // One connection and one database transaction. The statements run one after
  // another, in order; the first to fail rolls back every earlier one and ends
  // the transaction, so the failure is returned and no later statement runs.
  const transaction = async (
    endpoint: MySqlEndpoint,
    statements: readonly MySqlStatementParam[],
  ): Promise<MySqlTxOutcome> => {
    let conn: MySqlConnection | undefined;
    try {
      conn = await factory(connectionOptions(endpoint));
      await conn.beginTransaction();
      const runs: { rowCount: number; insertId: number }[] = [];
      try {
        for (const statement of statements) {
          const out = await query(conn, statement.sql, statement.params);
          runs.push({ rowCount: out.rowCount, insertId: out.insertId });
        }
        await conn.commit();
      } catch (e) {
        try {
          await conn.rollback();
        } catch {
          // The statement's failure is the outcome; a failed rollback is not another one.
        }
        throw e;
      }
      return { ok: true, runs };
    } catch (e) {
      return failed(e, endpoint);
    } finally {
      await close(conn);
    }
  };

  return Object.assign(execute, { transaction });
}

/** The executor a Workers build hands `WorkersMySql`, over the real driver. */
export const mysqlExecute: MySqlExecute = makeMySqlExecute(
  mysql2CreateConnection as unknown as MySqlFactory,
);
