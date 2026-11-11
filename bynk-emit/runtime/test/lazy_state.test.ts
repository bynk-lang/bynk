import { test } from "node:test";
import assert from "node:assert/strict";
import { canReadLazily, commitStored, noteClock, readStored, readsLazily } from "../src/entries.ts";
import type { EntryField } from "../src/entries.ts";
import { attachLazy } from "../src/lazy.ts";
import type { LazyFieldSpec } from "../src/lazy.ts";
import { StoreMap, StoreSet, wireMap, wirePostings } from "../src/store.ts";
import type { Indexer } from "../src/store.ts";
import { InMemoryStorage } from "../src/storage.ts";

// The lazy load and commit (#3189, the lazy-read slice): `readStored` takes the lazy read for a
// current-version per-entry record, `attachLazy` gives the state storage-backed handles, and
// `commitStored` flushes what they staged with the `"state"` record, in one transaction.

interface Item {
  team: string;
  n: number;
}
const FIELDS: EntryField[] = [
  { name: "items", kind: "map", indexes: [{ index: "by", recordField: "items__idx_by" }] },
  { name: "tags", kind: "set" },
  { name: "hits", kind: "cache" },
];
const BY: Indexer<Item> = { index: "by", of: (v) => v.team };
const SPECS: readonly LazyFieldSpec[] = [
  { name: "items", kind: "map" },
  { name: "tags", kind: "set" },
  { name: "hits", kind: "cache" },
];

interface State {
  count: number;
  items: StoreMap<Item>;
  tags: StoreSet;
  hits: unknown;
}

// What the emitted `loadState` and `commitState` do, over `storage`.
async function load(self: object, storage: InMemoryStorage, steps: Array<[number, number]> = []): Promise<State> {
  const stored = await storage.get<Record<string, unknown>>("state");
  const zero = { count: 0, items: new StoreMap<Item>([BY]), tags: new StoreSet() } as unknown as State;
  if (stored === undefined) return zero;
  const src = await readStored(self, storage, stored, steps);
  const merged = { ...zero, ...src } as State;
  if (readsLazily(self)) attachLazy(storage, merged, SPECS);
  return merged;
}

async function commit(self: object, storage: InMemoryStorage, s: State): Promise<void> {
  const encoded = { count: s.count, items: wireMap(s.items), items__idx_by: wirePostings(s.items, "by"), tags: {}, hits: {} };
  await commitStored(self, storage, encoded, FIELDS, s);
}

async function seed(n: number): Promise<InMemoryStorage> {
  const storage = new InMemoryStorage();
  const self = {};
  const s = await load(self, storage);
  s.count = 1;
  for (let i = 0; i < n; i += 1) await s.items.put(`k${i}`, { team: i % 2 === 0 ? "red" : "blue", n: i });
  await commit(self, storage, s);
  return storage;
}

test("a per-entry record reads lazily, and a wholesale or migrating one does not", async () => {
  assert.equal(canReadLazily({ __layout: 2 }, []), true);
  assert.equal(canReadLazily({}, []), false, "wholesale");
  assert.equal(canReadLazily({ __layout: 2, __v: 1 }, [[1, 1]]), false, "a step covers the stored version");
  assert.equal(canReadLazily({ __layout: 2, __v: 3 }, [[1, 1]]), true);
});

test("a one-entry write reads and writes a few keys, whatever the size of the collection", async () => {
  const counts: number[] = [];
  for (const n of [10, 1000]) {
    const storage = await seed(n);
    const self = {};
    storage.resetStats();
    const s = await load(self, storage);
    await s.items.put("k3", { team: "green", n: 99 });
    await commit(self, storage, s);
    counts.push(storage.stats.keysRead, storage.stats.keysWritten);
    assert.deepEqual(await storage.get("m/items/k3"), { i: 3, v: { team: "green", n: 99 } });
  }
  assert.ok(counts[0]! <= 6, `reads at N=10: ${counts[0]}`);
  assert.equal(counts[2], counts[0], "reads do not grow with N");
  assert.equal(counts[3], counts[1], "writes do not grow with N");
});

test("an indexed lookup reads the matching entries, not the collection", async () => {
  const storage = await seed(1000);
  const self = {};
  const s = await load(self, storage);
  storage.resetStats();
  const reds = await s.items.byIndex("by", "red");
  assert.equal(reds.length, 500);
  assert.ok(storage.stats.keysRead < 1100, "postings plus matches, not the whole record");
  const green = await s.items.byIndex("by", "green");
  assert.deepEqual(green, []);
});

test("a handler that faults commits nothing", async () => {
  const storage = await seed(5);
  const self = {};
  const s = await load(self, storage);
  await s.items.put("zz", { team: "red", n: 1 });
  // The handler throws before commit: nothing was written.
  assert.equal(await storage.get("m/items/zz"), undefined);
  const s2 = await load(self, storage);
  assert.equal(await s2.items.has("zz"), false);
});

test("a corrupt entry faults when touched, not when untouched", async () => {
  const storage = await seed(3);
  const specs: readonly LazyFieldSpec[] = [
    {
      name: "items",
      kind: "map",
      decode: (w) => {
        if ((w as { n: unknown }).n === 1) throw new Error("RehydrationViolation");
        return w;
      },
    },
  ];
  const self = {};
  const stored = (await storage.get<Record<string, unknown>>("state")) as Record<string, unknown>;
  const src = await readStored(self, storage, stored, []);
  const s = { ...src, items: new StoreMap<Item>([BY]) } as unknown as State;
  attachLazy(storage, s, specs);
  assert.equal((await s.items.get("k0"))?.n, 0, "an untouched corrupt entry does not fault");
  await assert.rejects(Promise.resolve(s.items.get("k1")), /RehydrationViolation/);
});

test("concurrent modifies of one key lose no update; different keys do not wait", async () => {
  const storage = await seed(3);
  const self = {};
  const s = await load(self, storage);
  await Promise.all([1, 2, 3, 4, 5].map(() => s.items.modify("k1", (c) => ({ team: c!.team, n: c!.n + 1 }))));
  assert.equal((await s.items.get("k1"))?.n, 1 + 5);
  await commit(self, storage, s);
  assert.deepEqual(await storage.get("m/items/k1"), { i: 1, v: { team: "blue", n: 6 } });
});

test("concurrent puts of one new key count it once", async () => {
  const storage = await seed(2);
  const self = {};
  const s = await load(self, storage);
  await Promise.all([s.items.put("new", { team: "red", n: 1 }), s.items.put("new", { team: "red", n: 2 })]);
  assert.equal(await s.items.size(), 3);
  await commit(self, storage, s);
  assert.deepEqual(await storage.get("c/items"), { size: 3, seq: 3 });
});

test("the commit reaps expired cache entries within its bound", async () => {
  const storage = new InMemoryStorage();
  await storage.put("state", { count: 0, __layout: 2 });
  for (let i = 0; i < 100; i += 1) {
    await storage.put(`m/hits/k${i}`, { v: i, exp: 10, i });
    await storage.put(`x/hits/${String(10).padStart(16, "0")}/k${i}`, 1);
  }
  await storage.put("c/hits", { size: 100, seq: 100, skew: 0 });
  const self = {};
  const s = await load(self, storage);
  noteClock(s, 50);
  await commit(self, storage, s);
  const left = [...(await storage.list({ prefix: "m/hits/" })).keys()].length;
  assert.equal(left, 100 - 64, "reaps at most 64 per commit");
});

test("a log record written before `skew` existed reads every entry, clamped ones included", async () => {
  const storage = new InMemoryStorage();
  await storage.put("state", { count: 0, __layout: 2 });
  // An entry appended at t=100 after one at t=900 took key time 900 (clamped); the record has no skew.
  const pad = (n: number): string => String(n).padStart(16, "0");
  await storage.put(`l/audit/${pad(900)}-0000000000`, { t: 900, v: "a" });
  await storage.put(`l/audit/${pad(900)}-0000000001`, { t: 100, v: "b" });
  await storage.put("c/audit", { size: 2, seq: 2 });
  const self = {};
  const stored = (await storage.get<Record<string, unknown>>("state")) as Record<string, unknown>;
  const src = await readStored(self, storage, stored, []);
  const s = { ...src } as Record<string, unknown>;
  attachLazy(storage, s, [{ name: "audit", kind: "log" }]);
  const log = s["audit"] as { before(t: number): Promise<readonly unknown[]>; between(a: number, b: number): Promise<readonly unknown[]>; since(t: number): Promise<readonly unknown[]> };
  assert.deepEqual(await log.before(200), ["b"], "the clamped entry is found below its key time");
  assert.deepEqual(await log.between(50, 150), ["b"]);
  assert.deepEqual(await log.since(-1_000_000), ["a", "b"], "a bound below zero reads from the start");
  assert.deepEqual(await log.before(-5), [], "nothing is older than a negative time");
});

test("a commit flushes the overlay of the state it commits, whatever else loaded in between", async () => {
  // Handlers on one instance are serialised in production (#3217); this pins that the commit does
  // not depend on it for finding its own writes: the context is found from the state's handles.
  const storage = await seed(2);
  const self = {};
  const a = await load(self, storage);
  await a.items.put("from-a", { team: "red", n: 1 });
  const b = await load(self, storage); // a second load on the same instance, before A commits
  await b.items.put("from-b", { team: "red", n: 2 });
  await commit(self, storage, a);
  assert.deepEqual(await storage.get("m/items/from-a"), { i: 2, v: { team: "red", n: 1 } }, "A's write is flushed by A's commit");
  assert.equal(await storage.get("m/items/from-b"), undefined, "B's write is not flushed by A's commit");
  await commit(self, storage, b);
  assert.ok((await storage.get("m/items/from-b")) !== undefined, "B's commit flushes B's write");
});

test("a key read back from storage is checked on every path that produces it", async () => {
  const storage = new InMemoryStorage();
  await storage.put("state", { count: 0, __layout: 2 });
  await storage.put("m/items/ok", { i: 0, v: { team: "red", n: 1 } });
  await storage.put("m/items/bad", { i: 1, v: { team: "red", n: 2 } });
  await storage.put("i/items/by/red/ok", 1);
  await storage.put("i/items/by/red/bad", 1);
  await storage.put("c/items", { size: 2, seq: 2, skew: 0 });
  await storage.put("s/tags/ok", { i: 0 });
  await storage.put("s/tags/bad", { i: 1 });
  await storage.put("c/tags", { size: 2, seq: 2 });
  const checkKey = (k: string): void => {
    if (k === "bad") throw new Error("RehydrationViolation");
  };
  const stored = (await storage.get<Record<string, unknown>>("state")) as Record<string, unknown>;
  const s = { ...(await readStored({}, storage, stored, [])), items: new StoreMap<Item>([BY]) } as unknown as State;
  attachLazy(storage, s, [
    { name: "items", kind: "map", checkKey },
    { name: "tags", kind: "set", checkKey },
  ]);
  const items = s.items as unknown as StoreMap<Item>;
  assert.equal((await items.get("ok"))?.n, 1, "a key the program supplies is not checked, and a good key reads");
  await assert.rejects(Promise.resolve(items.keys()), /RehydrationViolation/);
  await assert.rejects(Promise.resolve(items.values()), /RehydrationViolation/);
  await assert.rejects(Promise.resolve(items.entries()), /RehydrationViolation/);
  await assert.rejects(Promise.resolve(items.byIndex("by", "red")), /RehydrationViolation/);
  await assert.rejects(Promise.resolve((s.tags as unknown as StoreSet).keys()), /RehydrationViolation/);
});
