import { test } from "node:test";
import assert from "node:assert/strict";
import { classifyPgError, jsonColumn, makePgExecute, pgExecute, redactSecret } from "../src/postgres.ts";
import type { PgFactory, PgParam } from "../src/postgres.ts";

// Hyperdrive H1 (#3318, ADR 0799): the driver seam, tested against a fake
// `postgres` (the factory postgres.js's default export is). The fake records
// every call so the tests can read what reached the driver.

const URL_WITH_SECRET = "postgres://app:s3cr3t%21pw@db.internal.example:5432/shop";

interface Call {
  readonly sql: string;
  readonly params: readonly unknown[];
  readonly options: Record<string, unknown> | undefined;
}

function fake(answer: (call: Call) => unknown) {
  const calls: Call[] = [];
  // The order of what the fake saw, so a test can read a transaction's shape.
  const events: string[] = [];
  const opened: { url: string; options: Record<string, unknown> }[] = [];
  let ended = 0;
  const factory: PgFactory = (url, options) => {
    opened.push({ url, options });
    return {
      // Mirrors postgres.js 3.4.5's `workerd` `sql.unsafe`: without an options
      // argument the query is not prepared, which is the case the guard exists for.
      unsafe(sql: string, params: readonly unknown[], options: { prepare: boolean }) {
        const call: Call = { sql, params, options };
        calls.push(call);
        events.push(`unsafe ${sql}`);
        try {
          return Promise.resolve(answer(call) as never);
        } catch (e) {
          return Promise.reject(e);
        }
      },
      // Mirrors `sql.begin`: the callback's `tx.unsafe` is the same method; a
      // throw rolls back (and rethrows), a return commits.
      async begin<T>(fn: (tx: never) => Promise<T>): Promise<T> {
        events.push("begin");
        const tx = {
          unsafe: (sql: string, params: readonly unknown[], options: { prepare: boolean }) => {
            const call: Call = { sql, params, options };
            calls.push(call);
            events.push(`unsafe ${sql}`);
            try {
              return Promise.resolve(answer(call) as never);
            } catch (e) {
              return Promise.reject(e);
            }
          },
        };
        try {
          const out = await fn(tx as never);
          events.push("commit");
          return out;
        } catch (e) {
          events.push("rollback");
          throw e;
        }
      },
      async end() {
        ended += 1;
      },
    };
  };
  return { factory, calls, events, opened, ended: () => ended };
}

function rows(list: Record<string, unknown>[], count?: number) {
  return Object.assign([...list], { count: count ?? list.length });
}

const T = (value: string): PgParam => ({ tag: "PgText", value });

test("every sql.unsafe call passes { prepare: true } (ADR 0799 A)", async () => {
  const f = fake((call) => {
    // The guard: a call without the option would be unprepared on `workerd`.
    assert.deepEqual(call.options, { prepare: true });
    return rows([]);
  });
  const run = makePgExecute(f.factory);
  const params: PgParam[] = [{ tag: "PgInteger", value: 1 }, T("x")];
  await run(URL_WITH_SECRET, "select $1, $2", params);
  await run(URL_WITH_SECRET, "select 1", []);
  await run(URL_WITH_SECRET, "update t set a = 1", []);
  assert.equal(f.calls.length, 3);
  assert.ok(f.calls.every((c) => c.options?.prepare === true));
});

test("one connection per statement, opened with the connection string, ended afterwards", async () => {
  const f = fake(() => rows([]));
  const run = makePgExecute(f.factory);
  await run(URL_WITH_SECRET, "select 1", []);
  await run(URL_WITH_SECRET, "select 2", []);
  assert.equal(f.opened.length, 2);
  assert.equal(f.opened[0]!.url, URL_WITH_SECRET);
  assert.equal(f.opened[0]!.options.max, 1);
  assert.equal(f.opened[0]!.options.fetch_types, false);
  assert.equal(f.ended(), 2);
});

test("the connection is ended when the statement fails", async () => {
  const f = fake(() => {
    throw Object.assign(new Error("boom"), { code: "42601" });
  });
  const outcome = await makePgExecute(f.factory)(URL_WITH_SECRET, "selec", []);
  assert.equal(outcome.ok, false);
  assert.equal(f.ended(), 1);
});

test("a factory that throws is a failure, not a throw, and nothing is left open", async () => {
  const run = makePgExecute(() => {
    throw new Error(`Invalid URL ${URL_WITH_SECRET}`);
  });
  const outcome = await run(URL_WITH_SECRET, "select 1", []);
  assert.equal(outcome.ok, false);
  if (!outcome.ok) {
    assert.ok(!outcome.failure.message.includes("s3cr3t"));
    assert.ok(!outcome.failure.message.includes(URL_WITH_SECRET));
  }
});

test("parameters reach the driver in their driver form", async () => {
  const f = fake(() => rows([]));
  const bytes = new Uint8Array([1, 2, 3]);
  await makePgExecute(f.factory)(URL_WITH_SECRET, "select $1,$2,$3,$4,$5,$6,$7,$8,$9", [
    { tag: "PgNull" },
    { tag: "PgInteger", value: 7 },
    { tag: "PgReal", value: 1.5 },
    T("a"),
    { tag: "PgBytea", value: bytes },
    { tag: "PgBoolean", value: false },
    { tag: "PgTimestamp", value: 1_700_000_000_000 },
    { tag: "PgJson", value: '{"a":1}' },
    { tag: "PgNumeric", value: "12345678901234567890.5" },
  ]);
  const p = f.calls[0]!.params;
  assert.equal(p[0], null);
  assert.equal(p[1], 7);
  assert.equal(p[2], 1.5);
  assert.equal(p[3], "a");
  assert.equal(p[4], bytes);
  assert.equal(p[5], false);
  assert.deepEqual(p[6], new Date(1_700_000_000_000));
  assert.equal(p[7], '{"a":1}');
  assert.equal(p[8], "12345678901234567890.5");
});

test("rows are JSON-ready under ADR 0799 F, and the count is the driver's", async () => {
  const f = fake(() =>
    rows(
      [
        {
          id: 9007199254740993n,
          n: "12345678901234567890.5",
          at: new Date("2026-11-11T08:00:00.000Z"),
          raw: new Uint8Array([104, 105]),
          doc: { a: [1, { b: null }] },
          tags: ["x", "y"],
        },
      ],
      1,
    ),
  );
  const out = await makePgExecute(f.factory)(URL_WITH_SECRET, "select 1", []);
  assert.ok(out.ok);
  if (!out.ok) return;
  assert.equal(out.rowCount, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(out.rows)), [
    {
      id: "9007199254740993",
      n: "12345678901234567890.5",
      at: "2026-11-11T08:00:00.000Z",
      raw: "aGk=",
      doc: { a: [1, { b: null }] },
      tags: ["x", "y"],
    },
  ]);
});

test("a statement with no rows reports its count; a missing count falls back to the rows", async () => {
  const w = await makePgExecute(fake(() => rows([], 3)).factory)(URL_WITH_SECRET, "delete from t", []);
  assert.ok(w.ok && w.rowCount === 3);
  const none = await makePgExecute(fake(() => Object.assign([{ a: 1 }], { count: null })).factory)(URL_WITH_SECRET, "x", []);
  assert.ok(none.ok && none.rowCount === 1);
});

test("an invalid date column is null, not a throw", () => {
  assert.equal(jsonColumn(new Date(Number.NaN)), null);
});

test("SQLSTATE mapping: class 23 is constraint", () => {
  for (const code of ["23000", "23502", "23503", "23505", "23514"]) {
    const f = classifyPgError(Object.assign(new Error("dup"), { code }));
    assert.deepEqual([f.kind, f.sqlstate, f.message], ["constraint", code, "dup"]);
  }
});

test("SQLSTATE mapping: 40001 is serialization; 40P01 is not", () => {
  assert.equal(classifyPgError(Object.assign(new Error("s"), { code: "40001" })).kind, "serialization");
  assert.equal(classifyPgError(Object.assign(new Error("d"), { code: "40P01" })).kind, "failed");
});

test("SQLSTATE mapping: a connection code with no SQLSTATE is connection, with a fixed message", () => {
  for (const code of ["CONNECTION_CLOSED", "CONNECT_TIMEOUT", "ECONNREFUSED", "ENOTFOUND"]) {
    const f = classifyPgError(Object.assign(new Error("write CONNECTION_CLOSED db.internal.example:5432"), { code }));
    assert.equal(f.kind, "connection");
    assert.equal(f.sqlstate, "");
    assert.equal(f.message, `could not reach the database (${code})`);
    assert.ok(!f.message.includes("db.internal.example"));
  }
});

test("SQLSTATE mapping: everything else is failed, with the SQLSTATE where there is one", () => {
  assert.deepEqual(classifyPgError(Object.assign(new Error("syntax"), { code: "42601" })), {
    kind: "failed",
    sqlstate: "42601",
    message: "syntax",
  });
  assert.equal(classifyPgError(new Error("plain")).kind, "failed");
  assert.equal(classifyPgError("a string").kind, "failed");
  assert.equal(classifyPgError(null).kind, "failed");
  // A non-string code is no code.
  assert.equal(classifyPgError(Object.assign(new Error("x"), { code: 23505 })).kind, "failed");
});

test("the connection string and its password never reach a failure message", async () => {
  const f = fake(() => {
    throw Object.assign(new Error(`could not connect to ${URL_WITH_SECRET} as s3cr3t!pw`), { code: "XX000" });
  });
  const out = await makePgExecute(f.factory)(URL_WITH_SECRET, "select 1", []);
  assert.ok(!out.ok);
  if (out.ok) return;
  assert.ok(!out.failure.message.includes("s3cr3t"));
  assert.ok(!out.failure.message.includes(URL_WITH_SECRET));
  assert.equal(redactSecret("no secret here", "not a url"), "no secret here");
});

test("a password with a malformed percent-escape is still removed", () => {
  const url = "postgres://app:p%E0ss@db.internal.example/shop";
  const text = redactSecret("failed for p%E0ss and p%E0ss again", url);
  assert.ok(!text.includes("p%E0ss"), text);
});

test("the real default executor is postgres.js's, and reports an unreachable host as a connection failure", async () => {
  // Port 1 on loopback refuses at once. The point is the wiring: the real
  // package is imported and its connection error is classified, with the
  // password kept out of the message.
  const out = await pgExecute("postgres://app:s3cr3t@127.0.0.1:1/db", "select 1", []);
  assert.ok(!out.ok);
  if (out.ok) return;
  assert.equal(out.failure.kind, "connection");
  assert.ok(!out.failure.message.includes("s3cr3t"));
  assert.ok(!out.failure.message.includes("127.0.0.1"));
});

// Hyperdrive H3 (#3320): `transaction`.

test("a transaction runs its statements in order in one begin, each with { prepare: true }, and commits", async () => {
  const f = fake((call) => {
    assert.deepEqual(call.options, { prepare: true });
    return rows([], call.sql.startsWith("insert") ? 1 : 4);
  });
  const out = await makePgExecute(f.factory).transaction(URL_WITH_SECRET, [
    { sql: "insert into a values ($1)", params: [T("x")] },
    { sql: "update b set n = 1", params: [] },
  ]);
  assert.deepEqual(out, { ok: true, rowCounts: [1, 4] });
  assert.deepEqual(f.events, ["begin", "unsafe insert into a values ($1)", "unsafe update b set n = 1", "commit"]);
  assert.deepEqual(f.calls[0]!.params, ["x"]);
  assert.equal(f.opened.length, 1);
  assert.equal(f.ended(), 1);
});

test("a failing second statement rolls back the first and stops; the SQLSTATE is kept", async () => {
  const f = fake((call) => {
    if (call.sql === "insert into b values (1)") {
      throw Object.assign(new Error("duplicate key"), { code: "23505" });
    }
    return rows([], 1);
  });
  const out = await makePgExecute(f.factory).transaction(URL_WITH_SECRET, [
    { sql: "insert into a values (1)", params: [] },
    { sql: "insert into b values (1)", params: [] },
    { sql: "insert into c values (1)", params: [] },
  ]);
  assert.ok(!out.ok);
  if (out.ok) return;
  assert.deepEqual([out.failure.kind, out.failure.sqlstate], ["constraint", "23505"]);
  // Rolled back after the second statement; the third never ran.
  assert.deepEqual(f.events, ["begin", "unsafe insert into a values (1)", "unsafe insert into b values (1)", "rollback"]);
  assert.equal(f.ended(), 1);
});

test("a transaction's connection failure keeps the secret out of the message and ends the client", async () => {
  const f = fake(() => {
    throw Object.assign(new Error(`write CONNECTION_CLOSED ${URL_WITH_SECRET}`), { code: "CONNECTION_CLOSED" });
  });
  const out = await makePgExecute(f.factory).transaction(URL_WITH_SECRET, [{ sql: "select 1", params: [] }]);
  assert.ok(!out.ok);
  if (out.ok) return;
  assert.equal(out.failure.kind, "connection");
  assert.ok(!out.failure.message.includes("s3cr3t"));
  assert.equal(f.ended(), 1);
  const refused = await makePgExecute(() => {
    throw new Error(`Invalid URL ${URL_WITH_SECRET}`);
  }).transaction(URL_WITH_SECRET, []);
  assert.ok(!refused.ok);
  if (!refused.ok) assert.ok(!refused.failure.message.includes("s3cr3t"));
});

test("a transaction's parameters reach the driver in their driver form", async () => {
  const f = fake(() => rows([]));
  await makePgExecute(f.factory).transaction(URL_WITH_SECRET, [
    { sql: "select $1, $2", params: [{ tag: "PgNull" }, { tag: "PgTimestamp", value: 1_700_000_000_000 }] },
  ]);
  assert.equal(f.calls[0]!.params[0], null);
  assert.deepEqual(f.calls[0]!.params[1], new Date(1_700_000_000_000));
});

test("the real default executor's transaction reports an unreachable host as a connection failure", async () => {
  const out = await pgExecute.transaction("postgres://app:s3cr3t@127.0.0.1:1/db", [{ sql: "select 1", params: [] }]);
  assert.ok(!out.ok);
  if (out.ok) return;
  assert.equal(out.failure.kind, "connection");
  assert.ok(!out.failure.message.includes("s3cr3t"));
});
