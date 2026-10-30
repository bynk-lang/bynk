import { test } from "node:test";
import assert from "node:assert/strict";
import { makeIntegrationAnalytics, makeIntegrationD1, makeIntegrationKv, makeIntegrationQueue, makeIntegrationRateLimit } from "../src/storage.ts";

// Standard-library P3 (#2342): the integration tier's rate limiter.
test("makeIntegrationRateLimit allows `limit` calls per key per fixed window", async () => {
  let now = 0;
  const rl = makeIntegrationRateLimit(2, 10, () => now);
  const allow = async (key: string) => (await rl.limit({ key })).success;
  assert.deepEqual([await allow("a"), await allow("a"), await allow("a")], [true, true, false]);
  assert.equal(await allow("b"), true, "each key counts on its own");
  now = 9_999;
  assert.equal(await allow("a"), false, "still the same window");
  now = 10_000;
  assert.equal(await allow("a"), true, "a new window starts over");
});

test("makeIntegrationRateLimit stands still without a clock", async () => {
  const rl = makeIntegrationRateLimit(1, 10);
  assert.deepEqual([(await rl.limit({ key: "a" })).success, (await rl.limit({ key: "a" })).success], [true, false]);
});

test("makeIntegrationQueue keeps every body it is sent, in order", async () => {
  const q = makeIntegrationQueue();
  await q.send({ id: 1 }, { contentType: "json" });
  await q.sendBatch([{ body: "two" }, { body: [3] }]);
  assert.deepEqual(q.sent, [{ id: 1 }, "two", [3]]);
});

test("makeIntegrationAnalytics keeps every data point it is written, in order", () => {
  const a = makeIntegrationAnalytics();
  a.writeDataPoint({ indexes: ["k"], blobs: ["x"], doubles: [1] });
  a.writeDataPoint({ blobs: [], doubles: [] });
  assert.deepEqual(a.written, [{ indexes: ["k"], blobs: ["x"], doubles: [1] }, { blobs: [], doubles: [] }]);
});

test("makeIntegrationKv keeps text and bytes as one, with metadata, and pages in key order", async () => {
  const kv = makeIntegrationKv();
  await kv.put("a", "héllo", { metadata: { tag: "t" }, expirationTtl: 120 });
  await kv.put("b", new Uint8Array([0, 255]));
  await kv.put("c", "three");
  assert.equal(await kv.get("a"), "héllo");
  assert.deepEqual([...new Uint8Array((await kv.get("b", "arrayBuffer"))!)], [0, 255]);
  assert.deepEqual(await kv.getWithMetadata("a", { type: "text" }), { value: "héllo", metadata: { tag: "t" } });
  const first = await kv.list({ limit: 2 });
  assert.deepEqual(first.keys.map((k) => k.name), ["a", "b"]);
  assert.equal(first.list_complete, false);
  assert.ok(typeof first.keys[0].expiration === "number");
  const second = await kv.list({ limit: 2, cursor: first.cursor });
  assert.deepEqual(second.keys.map((k) => k.name), ["c"]);
  assert.equal(second.list_complete, true);
});

test("makeIntegrationD1 runs real SQL, binding integers as INTEGER", async () => {
  const d = makeIntegrationD1();
  await d.prepare("CREATE TABLE t (id INTEGER PRIMARY KEY, n INT, b BLOB)").run();
  const w = await d.prepare("INSERT INTO t (n, b) VALUES (?1, ?2)").bind(5, new Uint8Array([1, 2])).run();
  assert.deepEqual(w.meta, { changes: 1, last_row_id: 1 });
  const row = await d.prepare("SELECT typeof(n) AS ty, n, b FROM t").first();
  assert.equal(row?.ty, "integer");
  assert.equal(row?.n, 5);
  assert.deepEqual([...(row?.b as Uint8Array)], [1, 2]);
  assert.equal(await d.prepare("SELECT * FROM t WHERE n = ?").bind(9).first(), null);
});

test("makeIntegrationD1 words a constraint failure as D1 does", async () => {
  const d = makeIntegrationD1();
  await d.prepare("CREATE TABLE t (k TEXT UNIQUE)").run();
  await d.prepare("INSERT INTO t VALUES ('a')").run();
  await assert.rejects(d.prepare("INSERT INTO t VALUES ('a')").run(), /^Error: D1_ERROR: UNIQUE.*SQLITE_CONSTRAINT$/);
  await assert.rejects(d.prepare("SELEC 1").all(), (e: Error) => e.message.startsWith("D1_ERROR: ") && !e.message.includes("CONSTRAINT"));
});

test("makeIntegrationD1 runs a batch as one transaction", async () => {
  const d = makeIntegrationD1();
  await d.prepare("CREATE TABLE t (k TEXT UNIQUE)").run();
  const ok = await d.batch([d.prepare("INSERT INTO t VALUES (?)").bind("a"), d.prepare("INSERT INTO t VALUES (?)").bind("b")]);
  assert.deepEqual(ok.map((r) => r.meta.changes), [1, 1]);
  await assert.rejects(d.batch([d.prepare("INSERT INTO t VALUES ('c')"), d.prepare("INSERT INTO t VALUES ('a')")]));
  const all = await d.prepare("SELECT k FROM t ORDER BY k").all();
  assert.deepEqual(all.results.map((r) => r.k), ["a", "b"]);
});

test("makeIntegrationD1 binds an integral Real as INTEGER, as D1 does", async () => {
  // `Real(2.0)` is the number 2 by the time it is bound: D1 stores it as an
  // integer, and so does the double. A change in D1 here should change this.
  const d = makeIntegrationD1();
  const row = await d.prepare("SELECT typeof(?1) AS whole, typeof(?2) AS part, ?1 AS v").bind(2.0, 2.5).first();
  assert.deepEqual({ ...row }, { whole: "integer", part: "real", v: 2 });
});
