import { test } from "node:test";
import assert from "node:assert/strict";
import {
  EntryOverlay,
  LAYOUT_KEY,
  LAYOUT_PER_ENTRY,
  LAYOUT_WHOLESALE,
  decodeKeyPart,
  diffEntries,
  sameStored,
  encodeKeyPart,
  entryKeyOf,
  expiredRange,
  expiryKey,
  layoutOf,
  logKey,
  logRange,
  mapKey,
  mapPrefix,
  metaKey,
  padNumber,
  postingKey,
  postingPrefix,
  reindex,
  setKey,
  splitWholesale,
} from "../src/entries.ts";
import { InMemoryStorage, STORAGE_BATCH_LIMIT, utf8Compare } from "../src/storage.ts";

// Per-entry storage, the runtime half (#3189).

const keys = (m: Map<string, unknown>): string[] => [...m.keys()];

test("the key escape is reversible and keeps the separator out of a part", () => {
  for (const part of ["", "plain", "a/b", "100%", "%2F", "a/b%c/", "/", "%", "é/漢"]) {
    const enc = encodeKeyPart(part);
    assert.ok(!enc.includes("/"), `no separator in ${JSON.stringify(enc)}`);
    assert.equal(decodeKeyPart(enc), part);
  }
  // Two different keys never share a storage key, including across the escape.
  assert.notEqual(mapKey("f", "a/b"), mapKey("f", "a%2Fb"));
  assert.notEqual(mapKey("f", "a/b"), mapKey("f/a", "b"));
});

test("key shapes follow the layout", () => {
  assert.equal(mapKey("orders", "k1"), "m/orders/k1");
  assert.equal(setKey("tags", "x"), "s/tags/x");
  assert.equal(logKey("audit", 1700000000123, 7), "l/audit/0001700000000123-0000000007");
  assert.equal(postingKey("orders", "byCust", "c1", "o9"), "i/orders/byCust/c1/o9");
  assert.equal(postingPrefix("orders", "byCust", "c1"), "i/orders/byCust/c1/");
  assert.equal(postingPrefix("orders", "byCust"), "i/orders/byCust/");
  assert.equal(expiryKey("sessions", 1700000000000, "s1"), "x/sessions/0001700000000000/s1");
  assert.equal(metaKey("orders"), "c/orders");
  assert.equal(entryKeyOf(mapPrefix("orders"), mapKey("orders", "a/b")), "a/b");
});

test("padNumber is fixed width and refuses what would break the order", () => {
  assert.equal(padNumber(5, 4), "0005");
  assert.throws(() => padNumber(-1, 4), RangeError);
  assert.throws(() => padNumber(1.5, 4), RangeError);
  assert.throws(() => padNumber(10000, 4), RangeError);
});

test("log keys list in time order, then counter order, and ranges are half open", async () => {
  const s = new InMemoryStorage();
  // Written out of order, and across a digit-count change (9 -> 10 -> 100).
  for (const [t, seq] of [[100, 0], [9, 0], [10, 1], [10, 0], [9, 1]] as const) await s.put(logKey("l", t, seq), { t });
  const all = await s.list({ prefix: "l/l/" });
  assert.deepEqual(keys(all).map((k) => k.slice(4)), [
    "0000000000000009-0000000000",
    "0000000000000009-0000000001",
    "0000000000000010-0000000000",
    "0000000000000010-0000000001",
    "0000000000000100-0000000000",
  ]);
  const window = await s.list(logRange("l", 9, 100));
  assert.equal(window.size, 4, "from is inclusive, to is exclusive");
  assert.equal((await s.list(logRange("l", undefined, 10))).size, 2);
  assert.equal((await s.list(logRange("l", 10, undefined))).size, 3);
  const recent = await s.list({ ...logRange("l", undefined, undefined), reverse: true, limit: 2 });
  assert.deepEqual(keys(recent).map((k) => k.slice(4, 20)), ["0000000000000100", "0000000000000010"]);
});

test("expiry keys list in expiry order and expiredRange takes exp <= now", async () => {
  const s = new InMemoryStorage();
  for (const [exp, k] of [[50, "a"], [100, "b"], [101, "c"], [9, "d"]] as const) await s.put(expiryKey("c", exp, k), 1);
  const due = await s.list(expiredRange("c", 100));
  assert.deepEqual(keys(due).map((k) => entryKeyOf("x/c/", k).split("/")[1]), ["d", "a", "b"]);
});

test("a posting lookup reads only that value's postings", async () => {
  const s = new InMemoryStorage();
  for (let i = 0; i < 50; i += 1) await s.put(postingKey("o", "byCust", `c${i % 5}`, `o${i}`), 1);
  s.resetStats();
  const hits = await s.list({ prefix: postingPrefix("o", "byCust", "c3") });
  assert.equal(hits.size, 10);
  assert.equal(s.stats.keysRead, 10, "ten keys read of fifty");
  // A value whose text is a prefix of another's is not confused with it.
  await s.put(postingKey("o", "byCust", "c3x", "oz"), 1);
  assert.equal((await s.list({ prefix: postingPrefix("o", "byCust", "c3") })).size, 10);
});

test("overlay: reads see staged writes, nothing reaches storage before the flush", async () => {
  const s = new InMemoryStorage();
  await s.put("m/f/a", 1);
  await s.put("m/f/b", 2);
  const o = new EntryOverlay(s);
  o.put("m/f/a", 10);
  o.delete("m/f/b");
  o.put("m/f/c", 3);
  assert.equal(await o.get("m/f/a"), 10);
  assert.equal(await o.get("m/f/b"), undefined, "a deleted key reads as absent");
  assert.equal(await o.get("m/f/c"), 3);
  assert.equal(await s.get("m/f/a"), 1, "storage still holds the old value");
  assert.equal(await s.get("m/f/b"), 2);
  assert.equal(o.pending, 3);
  await o.flush();
  assert.equal(await s.get("m/f/a"), 10);
  assert.equal(await s.get("m/f/b"), undefined);
  assert.equal(await s.get("m/f/c"), 3);
  assert.equal(o.pending, 0);
});

test("overlay: a put after a delete and a delete after a put each win", async () => {
  const s = new InMemoryStorage();
  await s.put("k", 1);
  const o = new EntryOverlay(s);
  o.delete("k");
  o.put("k", 2);
  assert.equal(await o.get("k"), 2);
  o.put("j", 1);
  o.delete("j");
  assert.equal(await o.get("j"), undefined);
  await o.flush();
  assert.equal(await s.get("k"), 2);
  assert.equal(await s.get("j"), undefined);
});

test("overlay: a discarded overlay persists nothing", async () => {
  const s = new InMemoryStorage();
  await s.put("k", 1);
  const o = new EntryOverlay(s);
  o.put("k", 9);
  o.put("n", 1);
  o.delete("k");
  o.discard();
  await o.flush();
  assert.equal(await s.get("k"), 1);
  assert.equal(await s.get("n"), undefined);
});

test("overlay: list merges staged puts and deletes, in order, within the limit", async () => {
  const s = new InMemoryStorage();
  for (const k of ["m/f/a", "m/f/c", "m/f/e", "m/f/g"]) await s.put(k, k);
  const o = new EntryOverlay(s);
  o.delete("m/f/c");
  o.put("m/f/b", "b");
  o.put("m/f/z", "z");
  o.put("m/other/x", "x");
  assert.deepEqual(keys(await o.list({ prefix: "m/f/" })), ["m/f/a", "m/f/b", "m/f/e", "m/f/g", "m/f/z"]);
  // A bounded read still returns the first three of what the handler sees: the
  // deleted `c` does not take a place.
  assert.deepEqual(keys(await o.list({ prefix: "m/f/", limit: 3 })), ["m/f/a", "m/f/b", "m/f/e"]);
  assert.deepEqual(keys(await o.list({ prefix: "m/f/", limit: 2, reverse: true })), ["m/f/z", "m/f/g"]);
  assert.deepEqual(keys(await o.list({ prefix: "m/f/", start: "m/f/b", end: "m/f/g" })), ["m/f/b", "m/f/e"]);
});

test("overlay: a bounded list over many entries reads a bounded number of keys", async () => {
  const s = new InMemoryStorage();
  for (let i = 0; i < 1000; i += 1) await s.put(`m/f/${String(i).padStart(4, "0")}`, i);
  const o = new EntryOverlay(s);
  o.put("m/f/0001x", "new");
  s.resetStats();
  const got = await o.list({ prefix: "m/f/", limit: 5 });
  assert.equal(got.size, 5);
  assert.ok(s.stats.keysRead <= 5, `read ${s.stats.keysRead} keys of 1000`);
});

test("overlay: getMany batches storage reads within the platform limit", async () => {
  const s = new InMemoryStorage();
  const ks = Array.from({ length: 300 }, (_, i) => `m/f/${i}`);
  for (const k of ks) await s.put(k, k);
  const o = new EntryOverlay(s);
  o.delete("m/f/7");
  o.put("m/f/8", "staged");
  s.resetStats();
  const got = await o.getMany<string>(ks);
  assert.equal(got.size, 299);
  assert.equal(got.get("m/f/8"), "staged");
  assert.ok(!got.has("m/f/7"));
  assert.equal(s.stats.reads, Math.ceil(298 / STORAGE_BATCH_LIMIT), "three batched reads, not 298 single ones");
});

test("flush: a write-set past the batch limit is one atomic transaction", async () => {
  const s = new InMemoryStorage();
  const o = new EntryOverlay(s);
  for (let i = 0; i < 400; i += 1) o.put(`m/f/${i}`, i);
  s.resetStats();
  await o.flush({ state: { n: 1, [LAYOUT_KEY]: LAYOUT_PER_ENTRY } });
  assert.equal(s.stats.transactions, 1);
  assert.equal(s.size, 401);
  assert.deepEqual(await s.get("state"), { n: 1, __layout: 2 });
});

test("flush: a failing flush leaves storage as it was", async () => {
  const s = new InMemoryStorage();
  await s.put("k", "old");
  // The third batch of puts fails, after the delete and two batches already ran.
  const run = s.transaction.bind(s);
  s.transaction = ((closure) =>
    run(async (txn) => {
      let puts = 0;
      return closure({
        ...txn,
        put: (async (...args: unknown[]) => {
          puts += 1;
          if (puts === 3) throw new Error("boom");
          return (txn.put as (...a: unknown[]) => Promise<void>)(...args);
        }) as typeof txn.put,
      });
    })) as typeof s.transaction;
  const o = new EntryOverlay(s);
  for (let i = 0; i < 300; i += 1) o.put(`m/f/${i}`, i);
  o.delete("k");
  await assert.rejects(() => o.flush(), /boom/);
  assert.equal(await s.get("k"), "old", "the delete did not land");
  assert.equal(s.size, 1, "none of the earlier batches landed");
  assert.equal(o.pending, 301, "the overlay is left as it was");
});

test("flush: a write staged while a flush is awaiting survives it", async () => {
  const s = new InMemoryStorage();
  const o = new EntryOverlay(s);
  o.put("a", 1);
  const run = s.transaction.bind(s);
  s.transaction = ((closure) =>
    run(async (txn) => {
      const r = await closure(txn);
      o.put("late", 2); // staged while the flush is still awaiting its transaction
      o.put("a", 3); // and one that restages a key the flush wrote
      return r;
    })) as typeof s.transaction;
  await o.flush();
  assert.equal(o.pending, 2, "the late writes are still staged");
  s.transaction = run;
  await o.flush();
  assert.equal(await s.get("late"), 2);
  assert.equal(await s.get("a"), 3);
});

test("an undefined value is refused by the overlay and by the double, as the platform refuses it", async () => {
  const s = new InMemoryStorage();
  assert.throws(() => new EntryOverlay(s).put("k", undefined), TypeError);
  await assert.rejects(() => s.put("k", undefined), TypeError);
  await assert.rejects(() => s.put({ k: undefined }), TypeError);
  await assert.rejects(() => s.transaction(async (txn) => txn.put("k", undefined)), TypeError);
});

test("splitWholesale: a Cache entry with no numeric exp is carried over with no expiry key, as it never expired under wholesale", () => {
  const { entries } = splitWholesale({ c: { a: { v: 1 }, b: { v: 2, exp: "soon" }, d: { v: 3, exp: 9 } } }, [{ name: "c", kind: "cache" }]);
  assert.deepEqual(entries[mapKey("c", "a")], { v: 1 });
  assert.deepEqual(Object.keys(entries).filter((k) => k.startsWith("x/")), [expiryKey("c", 9, "d")]);
});

test("reindex moves a posting in the write-set, and does nothing when the value is unchanged", async () => {
  const s = new InMemoryStorage();
  await s.put(postingKey("o", "byCust", "c1", "o9"), 1);
  const o = new EntryOverlay(s);
  reindex(o, "o", "byCust", "o9", "c1", "c2");
  reindex(o, "o", "byCust", "o8", undefined, "c2");
  reindex(o, "o", "byCust", "o7", "c3", "c3");
  assert.equal(o.pending, 3);
  await o.flush();
  assert.deepEqual(keys(await s.list({ prefix: "i/o/byCust/" })), ["i/o/byCust/c2/o8", "i/o/byCust/c2/o9"]);
});

test("layoutOf reads the marker: absent is wholesale, a number is that layout, anything else is unknown", () => {
  assert.equal(layoutOf({}), LAYOUT_WHOLESALE);
  assert.equal(layoutOf({ __layout: LAYOUT_PER_ENTRY }), LAYOUT_PER_ENTRY);
  for (const bad of ["2", 0, -1, 1.5, null, Number.NaN]) assert.equal(layoutOf({ __layout: bad }), undefined, String(bad));
});

test("splitWholesale: collections and postings become per-entry writes, the rest stays", () => {
  const stored = {
    __v: 3,
    title: "cell",
    orders: { o1: { cust: "c1" }, "o/2": { cust: "c2" } },
    orders__byCust: { c1: ["o1"], c2: ["o/2"] },
    tags: { a: true, b: true },
    sessions: { s1: { v: "x", exp: 500 }, s2: { v: "y", exp: 100 } },
    audit: [{ t: 10, v: "a" }, { t: 10, v: "b" }, { t: 20, v: "c" }],
  };
  const before = JSON.stringify(stored);
  const { record, entries } = splitWholesale(stored, [
    { name: "orders", kind: "map", indexes: [{ index: "byCust", recordField: "orders__byCust" }] },
    { name: "tags", kind: "set" },
    { name: "sessions", kind: "cache" },
    { name: "audit", kind: "log" },
  ]);
  assert.equal(JSON.stringify(stored), before, "the input is not changed");
  assert.deepEqual(record, { __v: 3, title: "cell", __layout: 2 });
  assert.deepEqual(entries[mapKey("orders", "o1")], { cust: "c1" });
  assert.deepEqual(entries[mapKey("orders", "o/2")], { cust: "c2" });
  assert.equal(entries[postingKey("orders", "byCust", "c1", "o1")], 1);
  assert.equal(entries[postingKey("orders", "byCust", "c2", "o/2")], 1);
  assert.deepEqual(entries[metaKey("orders")], { size: 2 });
  assert.equal(entries[setKey("tags", "a")], 1);
  assert.deepEqual(entries[metaKey("tags")], { size: 2 });
  assert.deepEqual(entries[mapKey("sessions", "s1")], { v: "x", exp: 500 });
  assert.equal(entries[expiryKey("sessions", 500, "s1")], 1);
  assert.equal(entries[expiryKey("sessions", 100, "s2")], 1);
  assert.deepEqual(entries[logKey("audit", 10, 0)], { t: 10, v: "a" });
  assert.deepEqual(entries[logKey("audit", 10, 1)], { t: 10, v: "b" }, "same millisecond keeps array order");
  assert.deepEqual(entries[metaKey("audit")], { size: 3, seq: 3 });
  // orders: 2 entries + 2 postings + meta; tags: 2 + meta; sessions: 2 + 2 expiries + meta; audit: 3 + meta.
  assert.equal(Object.keys(entries).length, 5 + 3 + 5 + 4);
});

test("splitWholesale: an absent collection still sheds its posting record, and an empty one writes only its meta", () => {
  const { record, entries } = splitWholesale(
    { orders__byCust: { c1: ["o1"] }, tags: {} },
    [
      { name: "orders", kind: "map", indexes: [{ index: "byCust", recordField: "orders__byCust" }] },
      { name: "tags", kind: "set" },
    ],
  );
  assert.deepEqual(record, { __layout: 2 });
  assert.deepEqual(Object.keys(entries), [metaKey("tags")]);
});

test("a split record round-trips through storage and the overlay reads it back", async () => {
  const stored = { n: 1, orders: { o1: { cust: "c1" }, o2: { cust: "c1" }, o3: { cust: "c2" } }, orders__byCust: { c1: ["o1", "o2"], c2: ["o3"] } };
  const { record, entries } = splitWholesale(stored, [{ name: "orders", kind: "map", indexes: [{ index: "byCust", recordField: "orders__byCust" }] }]);
  const s = new InMemoryStorage();
  const o = new EntryOverlay(s);
  for (const [k, v] of Object.entries(entries)) o.put(k, v);
  await o.flush({ state: record });
  // An indexed read of one customer reads only that customer's postings and entries.
  s.resetStats();
  const postings = await new EntryOverlay(s).list({ prefix: postingPrefix("orders", "byCust", "c1") });
  const pks = keys(postings).map((k) => entryKeyOf(postingPrefix("orders", "byCust", "c1"), k));
  const found = await new EntryOverlay(s).getMany(pks.map((pk) => mapKey("orders", pk)));
  assert.deepEqual([...found.values()], [{ cust: "c1" }, { cust: "c1" }]);
  assert.equal(s.stats.keysRead, 4, "two postings and two entries, nothing else");
});

test("utf8Compare orders by UTF-8 bytes, not UTF-16 code units", () => {
  // U+FF5E (BMP) sorts before U+1F600 (astral) in UTF-8; by code unit it sorts after.
  assert.ok(utf8Compare("～", "\u{1f600}") < 0);
  assert.ok("～" > "\u{1f600}");
  assert.equal(utf8Compare("a", "a"), 0);
  assert.ok(utf8Compare("a", "ab") < 0);
});

test("diffEntries: only new, changed and removed keys are written", () => {
  const base = new Map<string, unknown>([
    ["m/f/a", { x: 1 }],
    ["m/f/b", { x: 2 }],
    ["m/f/c", { x: 3 }],
  ]);
  const next = { "m/f/a": { x: 1 }, "m/f/b": { x: 20 }, "m/f/d": { x: 4 } };
  const d = diffEntries(base, next);
  assert.deepEqual(d.puts, { "m/f/b": { x: 20 }, "m/f/d": { x: 4 } });
  assert.deepEqual(d.deletes, ["m/f/c"]);
  assert.deepEqual(diffEntries(new Map(Object.entries(next)), next), { puts: {}, deletes: [] });
});

test("sameStored compares plain data structurally, whatever the key order", () => {
  assert.ok(sameStored({ a: 1, b: [1, { c: 2 }] }, { b: [1, { c: 2 }], a: 1 }));
  assert.ok(!sameStored({ a: 1 }, { a: 1, b: undefined }));
  assert.ok(!sameStored([1, 2], [2, 1]));
  assert.ok(!sameStored({ a: 1 }, [1]));
  assert.ok(!sameStored(null, {}));
  assert.ok(sameStored(null, null));
});

// The codec: the stored record read and written per entry (#3189, slice 2).
import { commitStored, readStored } from "../src/entries.ts";
import type { EntryField } from "../src/entries.ts";

const FIELDS: EntryField[] = [
  { name: "users", kind: "map", indexes: [{ index: "byTeam", recordField: "users__idx_byTeam" }] },
  { name: "tags", kind: "set" },
  { name: "hits", kind: "cache" },
  { name: "audit", kind: "log" },
];

async function load(self: object, s: InMemoryStorage): Promise<Record<string, any>> {
  const stored = ((await s.get("state")) ?? {}) as Record<string, unknown>;
  return (await readStored(self, s as never, stored, FIELDS)) as Record<string, any>;
}

test("codec: a state commits per entry and reads back in insertion order, integer-like keys included", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, {
    __v: 1, n: 3,
    users: { b: { t: "x" }, "10": { t: "y" }, a: { t: "x" } },
    users__idx_byTeam: { x: ["b", "a"], y: ["10"] },
    tags: { z: true, y: true },
    hits: { k: { v: 1, exp: 50 } },
    audit: [{ t: 5, v: "p" }, { t: 5, v: "q" }],
  }, FIELDS);
  const state = (await s.get("state")) as Record<string, unknown>;
  assert.equal(state.__layout, LAYOUT_PER_ENTRY);
  assert.equal(state.users, undefined);
  assert.equal(state.n, 3);
  const back = await load({}, s);
  assert.deepEqual(Object.keys(back.users), ["10", "b", "a"].sort((p, q) => (p === "10" ? -1 : q === "10" ? 1 : 0)));
  // insertion order b, 10, a is kept in the stored `i`, so the postings read in it.
  assert.deepEqual(back.users__idx_byTeam.x, ["b", "a"]);
  assert.deepEqual(Object.keys(back.tags), ["z", "y"]);
  assert.deepEqual(back.hits, { k: { v: 1, exp: 50 } });
  assert.deepEqual(back.audit, [{ t: 5, v: "p" }, { t: 5, v: "q" }]);
});

test("codec: an overwrite keeps its place; a removed key inserted again goes to the end", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, { users: { a: 1, b: 2, c: 3 } }, FIELDS);
  let st = await load(self, s);
  st.users.a = 10; // overwrite
  await commitStored(self, s as never, st, FIELDS);
  st = await load(self, s);
  assert.deepEqual(Object.keys(st.users), ["a", "b", "c"]);
  delete st.users.a; // remove, then a later handler inserts it again
  await commitStored(self, s as never, st, FIELDS);
  st = await load(self, s);
  st.users.a = 11;
  await commitStored(self, s as never, st, FIELDS);
  st = await load(self, s);
  assert.deepEqual(Object.keys(st.users), ["b", "c", "a"]);
});

test("codec: a commit writes only the entries that changed", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, { users: { a: 1, b: 2, c: 3 } }, FIELDS);
  const st = await load(self, s);
  st.users.b = 9;
  s.resetStats();
  await commitStored(self, s as never, st, FIELDS);
  assert.ok(s.stats.keysWritten <= 3, `keys written: ${s.stats.keysWritten}`);
});

test("codec: a wholesale record is upgraded on read and still reads back the same", async () => {
  const s = new InMemoryStorage();
  await s.put("state", { __v: 1, users: { b: 1, a: 2 }, audit: [{ t: 1, v: "x" }] });
  const st = await load({}, s);
  assert.deepEqual(Object.keys(st.users), ["b", "a"]);
  assert.equal(((await s.get("state")) as any).__layout, LAYOUT_PER_ENTRY);
  assert.deepEqual((await load({}, s)).audit, [{ t: 1, v: "x" }]);
});

test("codec: a write that throws leaves storage and the base as they were", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, { users: { a: 1 } }, FIELDS);
  const st = await load(self, s);
  st.users.b = 2;
  const real = s.transaction.bind(s);
  (s as any).transaction = async () => { throw new Error("boom"); };
  await assert.rejects(commitStored(self, s as never, st, FIELDS), /boom/);
  (s as any).transaction = real;
  assert.deepEqual(Object.keys((await load({}, s)).users), ["a"]);
  await commitStored(self, s as never, st, FIELDS); // the base still says only `a` is stored
  assert.deepEqual(Object.keys((await load({}, s)).users), ["a", "b"]);
});

test("codec: a log keeps its order and prunes its oldest", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, { audit: [{ t: 1, v: "a" }, { t: 1, v: "b" }, { t: 2, v: "c" }] }, FIELDS);
  let st = await load(self, s);
  st.audit = [...st.audit.slice(1), { t: 3, v: "d" }];
  await commitStored(self, s as never, st, FIELDS);
  st = await load(self, s);
  assert.deepEqual(st.audit.map((e: any) => e.v), ["b", "c", "d"]);
});

test("codec: a value a handler mutates in place is written, not hidden by the base", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, { users: { a: { n: 1 } } }, FIELDS);
  const st = await load(self, s);
  st.users.a.n = 2; // in place, as a handler's lowered update does
  await commitStored(self, s as never, st, FIELDS);
  assert.deepEqual((await load({}, s)).users, { a: { n: 2 } });
});

test("codec: a collection the current fields do not list is joined as it was stored", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, { users: { a: 1 }, hits: { k: { v: 1, exp: 9 } }, tags: { z: true } }, FIELDS);
  // A later version has none of these fields; a migration step reads them.
  const st = (await readStored({}, s as never, ((await s.get("state")) ?? {}) as object, [])) as Record<string, any>;
  assert.deepEqual(st.users, { a: 1 });
  assert.deepEqual(st.hits, { k: { v: 1, exp: 9 } });
  assert.deepEqual(st.tags, { z: true });
  assert.equal(st.__layout, undefined);
});

test("codec: a Cell re-kinded to a collection keeps its inline value for a step to read", async () => {
  const s = new InMemoryStorage();
  await s.put("state", { __layout: 2, users: 42 });
  const st = (await readStored({}, s as never, ((await s.get("state")) ?? {}) as object, FIELDS)) as Record<string, any>;
  assert.equal(st.users, 42);
});

test("codec: a field re-kinded from a Map to a Set is joined as the Map it was, and committed as a Set", async () => {
  const s = new InMemoryStorage();
  const self = {};
  await load(self, s);
  await commitStored(self, s as never, { users: { a: 1, b: 2 } }, FIELDS);
  const asSet: EntryField[] = [{ name: "users", kind: "set" }];
  const st = (await readStored(self, s as never, ((await s.get("state")) ?? {}) as object, asSet)) as Record<string, any>;
  assert.deepEqual(st.users, { a: 1, b: 2 }); // what the step reads
  st.users = { a: true, b: true }; // what the step produces
  await commitStored(self, s as never, st, asSet);
  assert.deepEqual([...(await s.list({ prefix: "m/users/" })).keys()], []);
  assert.deepEqual(Object.keys(((await readStored({}, s as never, ((await s.get("state")) ?? {}) as object, asSet)) as Record<string, any>).users), ["a", "b"]);
});
