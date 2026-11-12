// Hyperdrive H4 (#3321): the opt-in `Postgres` test double, run against the real
// PGlite package. The fidelity cases are the ones the proposal named: SQLSTATE
// for a constraint violation, ADR 0799 F's value rules, and `transaction`'s
// rollback.
import assert from "node:assert/strict";
import { test } from "node:test";
import { makeIntegrationPostgres } from "../src/pglite.ts";
import type { PgParam } from "../src/postgres.ts";

const int = (value: number): PgParam => ({ tag: "PgInteger", value });
const text = (value: string): PgParam => ({ tag: "PgText", value });

// One binding name, so the database is started once and reset per `fresh` call.
const BINDING = "HYPERDRIVE_TEST";
const fresh = (schema = "") => makeIntegrationPostgres(BINDING, schema).execute;

test("the connection string of the double is not a database address", () => {
  const { env } = makeIntegrationPostgres(BINDING);
  assert.equal(env[BINDING]?.connectionString, "bynk-pglite://HYPERDRIVE_TEST");
});

test("a statement runs for real, and a case starts from the schema alone", async () => {
  const schema = "CREATE TABLE orders (id serial PRIMARY KEY, name text UNIQUE NOT NULL);";
  let pg = fresh(schema);
  const ran = await pg("", "insert into orders (name) values ($1)", [text("a")]);
  assert.deepEqual(ran, { ok: true, rows: [], rowCount: 1 });
  const read = await pg("", "select id, name from orders", []);
  assert.deepEqual(read, { ok: true, rows: [{ id: 1, name: "a" }], rowCount: 1 });
  // The next case sees none of it, and its sequence starts again.
  pg = fresh(schema);
  const again = await pg("", "insert into orders (name) values ($1) returning id", [text("a")]);
  assert.deepEqual(again, { ok: true, rows: [{ id: 1 }], rowCount: 1 });
});

test("a constraint violation is SQLSTATE 23505, a missing table 42P01, a syntax error 42601", async () => {
  const pg = fresh("CREATE TABLE t (id int PRIMARY KEY, u text UNIQUE);");
  await pg("", "insert into t (id, u) values (1, 'a')", []);
  for (const [sql, kind, state] of [
    ["insert into t (id, u) values (1, 'b')", "constraint", "23505"],
    ["insert into t (id, u) values (2, 'a')", "constraint", "23505"],
    ["insert into t (id) values (null)", "constraint", "23502"],
    ["select * from nope", "failed", "42P01"],
    ["selec 1", "failed", "42601"],
    ["select 1/0", "failed", "22012"],
  ] as const) {
    const out = await pg("", sql, []);
    assert.equal(out.ok, false, sql);
    if (!out.ok) {
      assert.equal(out.failure.kind, kind, sql);
      assert.equal(out.failure.sqlstate, state, sql);
    }
  }
});

test("values read as ADR 0799 F says: int8 and numeric as text, timestamptz as ISO, bytea as base64, json nested", async () => {
  const pg = fresh("CREATE TABLE v (b bigint, n numeric, ts timestamptz, d date, t timestamp, y bytea, j jsonb);");
  const ran = await pg(
    "",
    "insert into v values ($1, $2, $3, '2026-11-12', '2026-11-12 10:20:30', $4, $5)",
    [
      { tag: "PgText", value: "9007199254740993" },
      { tag: "PgNumeric", value: "12345678901234567890.123456789" },
      { tag: "PgTimestamp", value: Date.UTC(2026, 10, 12, 10, 20, 30, 123) },
      { tag: "PgBytea", value: new Uint8Array([0xde, 0xad, 0xbe, 0xef]) },
      { tag: "PgJson", value: '{"a":[1,2,{"b":null}],"s":"x"}' },
    ],
  );
  assert.equal(ran.ok, true);
  const read = await pg("", "select * from v", []);
  assert.deepEqual(read, {
    ok: true,
    rowCount: 1,
    rows: [
      {
        b: "9007199254740993",
        n: "12345678901234567890.123456789",
        ts: "2026-11-12T10:20:30.123Z",
        d: "2026-11-12",
        t: "2026-11-12 10:20:30",
        y: "3q2+7w==",
        j: { a: [1, 2, { b: null }], s: "x" },
      },
    ],
  });
});

test("a small int8 is still text, as a large one is", async () => {
  const pg = fresh("CREATE TABLE v (b bigint, n numeric, i int);");
  await pg("", "insert into v values (1, 2.50, 3)", []);
  assert.deepEqual(await pg("", "select b, n, i from v", []), {
    ok: true,
    rows: [{ b: "1", n: "2.50", i: 3 }],
    rowCount: 1,
  });
});

test("an infinite timestamptz stays text, and a PgNull binds null", async () => {
  const pg = fresh("CREATE TABLE v (ts timestamptz, x int);");
  await pg("", "insert into v values ('infinity', $1)", [{ tag: "PgNull" }]);
  assert.deepEqual(await pg("", "select ts, x from v", []), {
    ok: true,
    rows: [{ ts: "infinity", x: null }],
    rowCount: 1,
  });
});

test("a transaction applies every statement, or none: a failure rolls the earlier ones back", async () => {
  const pg = fresh("CREATE TABLE t (id int PRIMARY KEY);");
  const ok = await pg.transaction("", [
    { sql: "insert into t values ($1)", params: [int(1)] },
    { sql: "insert into t values ($1)", params: [int(2)] },
  ]);
  assert.deepEqual(ok, { ok: true, rowCounts: [1, 1] });
  const bad = await pg.transaction("", [
    { sql: "insert into t values ($1)", params: [int(3)] },
    { sql: "insert into t values ($1)", params: [int(1)] },
  ]);
  assert.equal(bad.ok, false);
  if (!bad.ok) assert.equal(bad.failure.sqlstate, "23505");
  const count = await pg("", "select count(*)::int as c from t", []);
  assert.deepEqual(count, { ok: true, rows: [{ c: 2 }], rowCount: 1 });
});

test("a schema that fails faults the case, naming schema.sql, never a statement's Err", async () => {
  const pg = fresh("CREATE TABL broken");
  await assert.rejects(pg("", "select 1", []), /`schema\.sql` failed/);
  // And the next case, with a good schema, is unaffected.
  assert.equal((await fresh("CREATE TABLE ok (x int);")("", "select count(*)::int c from ok", [])).ok, true);
});
