import { test } from "node:test";
import assert from "node:assert/strict";
import {
  classifyMySqlError,
  connectionOptions,
  makeMySqlExecute,
  mysqlJsonColumn,
  mysqlTypeCast,
  redactEndpoint,
  utcDateTimeText,
} from "../src/mysql.ts";
import type { MySqlEndpoint, MySqlFactory, MySqlParam } from "../src/mysql.ts";

// Hyperdrive H5 (#3322, ADR 0799 D): the driver seam, tested against a fake
// `mysql2/promise` (the `createConnection` its default export has). The fake
// records every call so the tests can read what reached the driver.

const ENDPOINT: MySqlEndpoint = {
  host: "db.internal.example",
  port: 3306,
  user: "app",
  password: "s3cr3t!pw",
  database: "shop",
};

class DriverError extends Error {
  errno?: number;
  code?: string;
  constructor(message: string, errno?: number, code?: string) {
    super(message);
    this.errno = errno;
    this.code = code;
  }
}

function fake(answer: (sql: string, values: readonly unknown[]) => unknown) {
  const events: string[] = [];
  const queries: { sql: string; values: readonly unknown[] }[] = [];
  const opened: Record<string, unknown>[] = [];
  let ended = 0;
  const factory: MySqlFactory = async (options) => {
    opened.push(options);
    return {
      async query(sql: string, values: readonly unknown[]) {
        events.push(`query ${sql}`);
        queries.push({ sql, values });
        return [answer(sql, values), []] as [unknown, unknown];
      },
      async beginTransaction() {
        events.push("begin");
      },
      async commit() {
        events.push("commit");
      },
      async rollback() {
        events.push("rollback");
      },
      async end() {
        ended += 1;
        events.push("end");
      },
    };
  };
  return { factory, events, queries, opened, ended: () => ended };
}

test("every connection is opened with disableEval: true (Workers forbid eval)", async () => {
  const f = fake(() => []);
  const exec = makeMySqlExecute(f.factory);
  await exec(ENDPOINT, "SELECT 1", []);
  await exec.transaction(ENDPOINT, [{ sql: "SELECT 1", params: [] }]);
  assert.equal(f.opened.length, 2);
  for (const options of f.opened) {
    assert.equal(options.disableEval, true);
    assert.equal(options.multipleStatements, false);
    assert.equal(options.host, ENDPOINT.host);
    assert.equal(options.password, ENDPOINT.password);
  }
  assert.equal(connectionOptions(ENDPOINT).disableEval, true);
});

test("a statement runs through query with its ? parameters and the connection is ended", async () => {
  const f = fake(() => [{ id: 1, name: "a" }]);
  const exec = makeMySqlExecute(f.factory);
  const params: MySqlParam[] = [
    { tag: "MySqlInteger", value: 7 },
    { tag: "MySqlUnsigned", value: "18446744073709551615" },
    { tag: "MySqlBoolean", value: true },
    { tag: "MySqlNull" },
    { tag: "MySqlTimestamp", value: Date.UTC(2026, 10, 12, 9, 30, 0, 5) },
    { tag: "MySqlDateTime", value: "2026-11-12 09:30:00" },
    { tag: "MySqlJson", value: '{"a":1}' },
    { tag: "MySqlDecimal", value: "12345678901234567890.5" },
  ];
  const out = await exec(ENDPOINT, "SELECT * FROM t WHERE a = ? LIMIT ?, ?", params);
  assert.equal(out.ok, true);
  assert.deepEqual(f.queries[0].values, [
    7,
    "18446744073709551615",
    1,
    null,
    "2026-11-12 09:30:00.005",
    "2026-11-12 09:30:00",
    '{"a":1}',
    "12345678901234567890.5",
  ]);
  if (out.ok) assert.deepEqual(out.rows, [{ id: 1, name: "a" }]);
  assert.equal(f.ended(), 1);
});

test("a write answers its affected rows and insert id", async () => {
  const f = fake(() => ({ affectedRows: 2, insertId: 41 }));
  const out = await makeMySqlExecute(f.factory)(ENDPOINT, "INSERT INTO t VALUES (?)", [{ tag: "MySqlText", value: "x" }]);
  assert.deepEqual(out, { ok: true, rows: [], rowCount: 2, insertId: 41 });
});

test("bytes bind as a Buffer and read back as base64", async () => {
  const f = fake(() => [{ b: new Uint8Array([1, 2, 3]) }]);
  const out = await makeMySqlExecute(f.factory)(ENDPOINT, "SELECT ?", [{ tag: "MySqlBytes", value: new Uint8Array([1, 2, 3]) }]);
  assert.equal(Buffer.isBuffer(f.queries[0].values[0]), true);
  if (out.ok) assert.deepEqual(out.rows, [{ b: "AQID" }]);
});

test("typeCast: TINYINT(1) is a flag, dates stay text, the rest is the driver's", () => {
  const field = (type: string, length: number, text: string | null) => ({ type, length, string: () => text });
  assert.equal(mysqlTypeCast(field("TINY", 1, "1"), () => "next"), true);
  assert.equal(mysqlTypeCast(field("TINY", 1, "0"), () => "next"), false);
  assert.equal(mysqlTypeCast(field("TINY", 1, null), () => "next"), null);
  assert.equal(mysqlTypeCast(field("TINY", 4, "5"), () => "next"), "next");
  assert.equal(mysqlTypeCast(field("DATETIME", 19, "2026-11-12 09:30:00"), () => "next"), "2026-11-12 09:30:00");
  assert.equal(mysqlTypeCast(field("TIMESTAMP", 19, "2026-11-12 09:30:00"), () => "next"), "2026-11-12 09:30:00");
  assert.equal(mysqlTypeCast(field("LONGLONG", 20, "1"), () => "next"), "next");
  assert.equal(connectionOptions(ENDPOINT).bigNumberStrings, true);
});

test("errors are keyed on the MySQL error number", () => {
  for (const errno of [1062, 1048, 1452, 1451, 3819]) {
    assert.equal(classifyMySqlError(new DriverError("x", errno)).kind, "constraint", String(errno));
  }
  for (const errno of [1213, 1205]) {
    assert.equal(classifyMySqlError(new DriverError("x", errno)).kind, "serialization", String(errno));
  }
  const refused = classifyMySqlError(new DriverError("connect ECONNREFUSED 10.0.0.5:3306", undefined, "ECONNREFUSED"));
  assert.deepEqual(refused, { kind: "connection", code: 0, message: "could not reach the database (ECONNREFUSED)" });
  assert.equal(classifyMySqlError(new DriverError("gone", 2013, "PROTOCOL_CONNECTION_LOST")).kind, "connection");
  assert.equal(classifyMySqlError(new Error("connection attempt failed")).kind, "connection");
  const other = classifyMySqlError(new DriverError("You have an error in your SQL syntax", 1064));
  assert.deepEqual(other, { kind: "failed", code: 1064, message: "You have an error in your SQL syntax" });
  assert.deepEqual(classifyMySqlError("odd"), { kind: "failed", code: 0, message: "odd" });
});

test("a failure never carries the password or the host", async () => {
  const f = fake(() => {
    throw new DriverError(`Access denied for 'app' using password s3cr3t!pw`, 1045);
  });
  const out = await makeMySqlExecute(f.factory)(ENDPOINT, "SELECT 1", []);
  assert.equal(out.ok, false);
  assert.doesNotMatch(JSON.stringify(out), /s3cr3t/);
  assert.equal(f.ended(), 1);
  const conn = await makeMySqlExecute(async () => {
    throw new DriverError(`connect ECONNREFUSED ${ENDPOINT.host}:3306`, undefined, "ECONNREFUSED");
  })(ENDPOINT, "SELECT 1", []);
  assert.doesNotMatch(JSON.stringify(conn), /db\.internal/);
  assert.equal(redactEndpoint("pw s3cr3t!pw and s3cr3t!pw", ENDPOINT), "pw [redacted] and [redacted]");
});

test("real access-denied and denied-command texts never reach the handler with the user or host", async () => {
  const texts: [number, string][] = [
    [1045, "Access denied for user 'app'@'10.0.0.5' (using password: YES)"],
    [1044, "Access denied for user 'app'@'%' to database 'shop'"],
    [1049, "Unknown database 'shop'"],
    [1142, "SELECT command denied to user 'app'@'10.0.0.5' for table 'orders'"],
    [1130, "Host 'db.internal.example' is not allowed to connect to this MySQL server"],
  ];
  for (const [errno, text] of texts) {
    const f = fake(() => {
      throw new DriverError(text, errno);
    });
    const out = await makeMySqlExecute(f.factory)(ENDPOINT, "SELECT 1", []);
    assert.equal(out.ok, false);
    const shown = JSON.stringify(out);
    assert.doesNotMatch(shown, /10\.0\.0\.5|db\.internal|'app'|\bapp\b/, `${errno}: ${shown}`);
    if (!out.ok && [1044, 1045, 1049].includes(errno)) assert.equal(out.failure.kind, "connection");
  }
});

test("a transaction commits all or rolls back at the first failure", async () => {
  const ok = fake(() => ({ affectedRows: 1, insertId: 0 }));
  const done = await makeMySqlExecute(ok.factory).transaction(ENDPOINT, [
    { sql: "A", params: [] },
    { sql: "B", params: [] },
  ]);
  assert.deepEqual(done, { ok: true, runs: [{ rowCount: 1, insertId: 0 }, { rowCount: 1, insertId: 0 }] });
  assert.deepEqual(ok.events, ["begin", "query A", "query B", "commit", "end"]);

  const bad = fake((sql) => {
    if (sql === "B") throw new DriverError("Duplicate entry", 1062);
    return { affectedRows: 1, insertId: 0 };
  });
  const out = await makeMySqlExecute(bad.factory).transaction(ENDPOINT, [
    { sql: "A", params: [] },
    { sql: "B", params: [] },
    { sql: "C", params: [] },
  ]);
  assert.equal(out.ok, false);
  assert.deepEqual(bad.events, ["begin", "query A", "query B", "rollback", "end"]);
});

test("columns leave as JSON: bigint as text, nested JSON kept, dates as ISO", () => {
  assert.equal(mysqlJsonColumn(10n), "10");
  assert.deepEqual(mysqlJsonColumn({ a: [1n] }), { a: ["1"] });
  assert.equal(utcDateTimeText(0), "1970-01-01 00:00:00.000");
});
