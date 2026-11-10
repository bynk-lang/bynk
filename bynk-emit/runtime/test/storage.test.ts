import { test } from "node:test";
import assert from "node:assert/strict";
import { makeIntegrationAnalytics, makeIntegrationD1, makeIntegrationKv, makeIntegrationQueue, makeIntegrationR2, makeIntegrationRateLimit } from "../src/storage.ts";

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

test("makeIntegrationD1 refuses a wrong parameter count, as D1 does (#3003)", async () => {
  const d = makeIntegrationD1();
  await d.prepare("CREATE TABLE t (a, b)").run();
  const wrong = /^Error: D1_ERROR: Wrong number of parameter bindings for SQL query\.$/;
  const refused = (sql: string, ...v: unknown[]) => assert.rejects(d.prepare(sql).bind(...v).all(), wrong);
  const accepted = (sql: string, ...v: unknown[]) => d.prepare(sql).bind(...v).all();
  await refused("SELECT 1 WHERE ?1 = 1", 1, 2); // too many
  await refused("SELECT 1 WHERE ?1 = ?2", 1); // too few
  await refused("SELECT 1 WHERE ?2 = 1", 1); // ?2 needs two
  await refused("SELECT ?");
  await refused("SELECT 1", 1);
  await accepted("SELECT 1 WHERE ?1 = ?1", 1); // a repeat is one
  await accepted("SELECT 1 WHERE ? = ?", 1, 1);
  await accepted("SELECT 1 WHERE ?2 = ?", 1, 1, 1); // a bare ? follows the highest
  await refused("SELECT 1 WHERE ?2 = ?", 1, 1);
  await accepted("SELECT 1 WHERE '?1' = ?2", 1, 1); // quoted ?1 is no parameter
  await accepted("SELECT 1 WHERE ?3 = 1", 1, 1, 1); // the highest number
  await accepted("SELECT '?' AS \"?\", 2 AS `?`, 3 AS [?] -- ?\n /* ? */ WHERE 'it''s ?' = ?1", 1);
  await accepted("SELECT 1 AS a$b"); // `$` inside an identifier is no parameter
  await accepted("SELECT 1 AS é$b"); // #3157: nor after a non-ASCII identifier character
  await accepted("SELECT 1 WHERE ?01 = 1", 1); // the raw spelling is the bind key
  await refused("SELECT :a, :a, @b, $c", 1, 2);
  // #3157: a non-ASCII byte is part of a name, as in SQLite's tokenizer.
  // (Node binds no value to a named parameter by position, so these show the count only: two names, not one `:caf`; and `:é` is a name.)
  await refused("SELECT :café, :cafè", 1);
  await refused("SELECT :é");
  await assert.rejects(d.batch([d.prepare("SELECT ?1").bind(1, 2)]), wrong);
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

test("makeIntegrationD1 runs its migrations in order before it is used", async () => {
  const d = makeIntegrationD1([
    ["0001_create.sql", "CREATE TABLE t (k TEXT UNIQUE);\n-- a comment\n"],
    ["0002_more.sql", "ALTER TABLE t ADD COLUMN n INT DEFAULT 7;\nINSERT INTO t (k) VALUES ('seed');"],
  ]);
  const row = await d.prepare("SELECT k, n FROM t").first();
  assert.deepEqual({ ...row }, { k: "seed", n: 7 });
});

test("makeIntegrationD1 faults on a failing migration, naming it", () => {
  assert.throws(
    () => makeIntegrationD1([["0001_bad.sql", "CREATE TABLE t (k TEXT); CREATE TABL u (x)"]]),
    /^Error: bynk: D1 migration `0001_bad\.sql` failed: /,
  );
});

// P5b (#2626): the integration tier's R2 bucket streams bodies, takes a stream
// to `put`, and completes multipart uploads by R2's part rules.
test("makeIntegrationR2: get streams the body in 64 KiB chunks; put takes a stream", async () => {
  const r2 = makeIntegrationR2();
  const big = new Uint8Array(70_000).fill(7);
  await r2.put("k", new Response(big).body!);
  const o = await r2.get("k");
  const sizes: number[] = [];
  for await (const c of o!.body as unknown as AsyncIterable<Uint8Array>) sizes.push(c.length);
  assert.deepEqual(sizes, [65_536, 70_000 - 65_536]);
});

test("makeIntegrationR2: a multipart upload joins its parts in number order, by R2's rules", async () => {
  const r2 = makeIntegrationR2();
  const u = await r2.createMultipartUpload("big", { httpMetadata: { contentType: "text/plain" } });
  const resumed = r2.resumeMultipartUpload("big", u.uploadId);
  const p2 = await resumed.uploadPart(2, new Uint8Array([3]));
  const p1 = await resumed.uploadPart(1, new Uint8Array([1, 2]));
  await assert.rejects(resumed.uploadPart(0, new Uint8Array([1])), /not between 1 and 10,000/);
  await assert.rejects(resumed.complete([p1, { partNumber: 2, etag: "wrong" }]), /not uploaded with etag wrong/);
  const o = await resumed.complete([p2, p1]);
  assert.equal(o.size, 3);
  assert.equal(o.httpMetadata?.contentType, "text/plain");
  assert.deepEqual([...new Uint8Array(await (await r2.get("big"))!.arrayBuffer())], [1, 2, 3]);
  await assert.rejects(resumed.complete([p1]), /no multipart upload/);
});

test("makeIntegrationR2: every part but the last must be the same size", async () => {
  const r2 = makeIntegrationR2();
  const u = await r2.createMultipartUpload("big");
  const parts = [
    await u.uploadPart(1, new Uint8Array(2)),
    await u.uploadPart(2, new Uint8Array(3)),
    await u.uploadPart(3, new Uint8Array(1)),
  ];
  await assert.rejects(u.complete(parts), /must be the same size/);
  await u.abort();
  await assert.rejects(u.complete(parts), /no multipart upload/);
});

// Per-entry storage slice 1 (#3189): the batched, ranged and transactional forms.
import { InMemoryStorage, STORAGE_BATCH_LIMIT } from "../src/storage.ts";

test("InMemoryStorage: batched get, put and delete, in key order", async () => {
  const s = new InMemoryStorage();
  await s.put({ b: 2, a: 1, c: 3 });
  assert.deepEqual([...(await s.get<number>(["c", "a", "zz"]))], [["a", 1], ["c", 3]]);
  assert.equal(await s.delete(["a", "b", "nope"]), 2);
  assert.equal(await s.get("a"), undefined);
  assert.equal(await s.get("c"), 3);
});

test("InMemoryStorage: a batch past the platform limit is refused, as Cloudflare documents", async () => {
  const s = new InMemoryStorage();
  const many = Array.from({ length: STORAGE_BATCH_LIMIT + 1 }, (_, i) => `k${i}`);
  await assert.rejects(() => s.get(many), RangeError);
  await assert.rejects(() => s.delete(many), RangeError);
  await assert.rejects(() => s.put(Object.fromEntries(many.map((k) => [k, 1]))), RangeError);
  // Exactly the limit is fine.
  await s.put(Object.fromEntries(many.slice(0, STORAGE_BATCH_LIMIT).map((k) => [k, 1])));
  assert.equal(s.size, STORAGE_BATCH_LIMIT);
});

test("InMemoryStorage: list takes prefix, start, startAfter, end, limit and reverse", async () => {
  const s = new InMemoryStorage();
  for (const k of ["a", "b", "c", "d", "e", "x1"]) await s.put(k, k);
  const l = async (o: object): Promise<string[]> => [...(await s.list(o)).keys()];
  assert.deepEqual(await l({ start: "b", end: "e" }), ["b", "c", "d"]);
  assert.deepEqual(await l({ startAfter: "b", end: "e" }), ["c", "d"]);
  assert.deepEqual(await l({ end: "c", reverse: true }), ["b", "a"]);
  assert.deepEqual(await l({ limit: 2 }), ["a", "b"]);
  assert.deepEqual(await l({ limit: 2, reverse: true }), ["x1", "e"]);
  assert.deepEqual(await l({ prefix: "x" }), ["x1"]);
});

test("InMemoryStorage: a transaction commits together, and a throw or rollback commits nothing", async () => {
  const s = new InMemoryStorage();
  await s.put("keep", 1);
  const r = await s.transaction(async (txn) => {
    await txn.put({ a: 1, b: 2 });
    await txn.delete("keep");
    assert.equal(await txn.get("a"), 1, "a transaction reads its own writes");
    assert.equal(await txn.get("keep"), undefined);
    assert.equal(await s.get("a"), undefined, "nothing is visible outside before it commits");
    return "done";
  });
  assert.equal(r, "done");
  assert.deepEqual([await s.get("a"), await s.get("b"), await s.get("keep")], [1, 2, undefined]);

  await assert.rejects(
    () =>
      s.transaction(async (txn) => {
        await txn.put("c", 1);
        throw new Error("boom");
      }),
    /boom/,
  );
  await s.transaction(async (txn) => {
    await txn.put("d", 1);
    txn.rollback();
  });
  assert.deepEqual([await s.get("c"), await s.get("d")], [undefined, undefined]);
  // A failed transaction does not wedge the storage.
  await s.put("after", 1);
  assert.equal(await s.get("after"), 1);
});

test("InMemoryStorage: transactions cannot nest or overlap", async () => {
  const s = new InMemoryStorage();
  await assert.rejects(() => s.transaction(async () => s.transaction(async () => 1)), /cannot overlap or nest/);
});

test("InMemoryStorage: it counts operations so a test can assert I/O does not grow with state", async () => {
  const s = new InMemoryStorage();
  for (let i = 0; i < 100; i += 1) await s.put(`k${i}`, i);
  s.resetStats();
  await s.get("k1");
  await s.put("k1", 2);
  await s.list({ prefix: "k9" });
  assert.deepEqual(
    { reads: s.stats.reads, writes: s.stats.writes, lists: s.stats.lists, keysRead: s.stats.keysRead, keysWritten: s.stats.keysWritten },
    { reads: 1, writes: 1, lists: 1, keysRead: 1 + 11, keysWritten: 1 },
  );
});
