import { test } from "node:test";
import assert from "node:assert/strict";
import { EntryOverlay, commitStored, expiredRange, noteClock, readStored } from "../src/entries.ts";
import type { EntryField } from "../src/entries.ts";
import { LazyStoreCache, LazyStoreLog, LazyStoreMap, LazyStoreSet } from "../src/lazy.ts";
import { StoreCache, StoreLog, StoreMap, StoreSet, fillLog, fillMap, fillSet, wireLog, wireMap, wirePostings, wireSet } from "../src/store.ts";
import type { CacheEntry, Indexer } from "../src/store.ts";
import { InMemoryStorage } from "../src/storage.ts";

// The storage-backed handles against the in-memory ones (#3189, the lazy-read slice): the same
// operations through both must read the same, and leave byte-identical stored state.

interface Item {
  team: string;
  n: number;
}

const FIELDS: EntryField[] = [
  { name: "items", kind: "map", indexes: [{ index: "by", recordField: "items__idx_by" }] },
  { name: "tags", kind: "set" },
  { name: "hits", kind: "cache" },
  { name: "audit", kind: "log" },
];
const BY: Indexer<Item> = { index: "by", of: (v) => v.team };

// A small seeded generator, so a failing run can be replayed.
function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 4294967296;
  };
}

const KEYS = ["10", "2", "a", "5", "b", "zz", "a/b", "%"];
const TEAMS = ["red", "blue", "green"];

interface Eager {
  items: StoreMap<Item>;
  tags: StoreSet;
  hits: StoreCache<number>;
  audit: StoreLog<number>;
}

async function loadEager(self: object, s: InMemoryStorage): Promise<Eager> {
  const st = (await readStored(self, s as never, ((await s.get("state")) ?? {}) as object)) as Record<string, any>;
  const e: Eager = { items: new StoreMap<Item>([BY]), tags: new StoreSet(), hits: new StoreCache<number>(), audit: new StoreLog<number>() };
  if (st.items) fillMap(e.items, st.items);
  if (st.tags) fillSet(e.tags, st.tags);
  if (st.hits) fillMap(e.hits as never, st.hits);
  if (st.audit) fillLog(e.audit, st.audit);
  return e;
}

async function commitEager(self: object, s: InMemoryStorage, e: Eager, state?: object): Promise<void> {
  const encoded = {
    items: wireMap(e.items),
    items__idx_by: wirePostings(e.items, "by"),
    tags: wireSet(e.tags),
    hits: wireMap(e.hits as never),
    audit: wireLog(e.audit),
  };
  await commitStored(self, s as never, encoded, FIELDS, state);
}

interface Lazy {
  overlay: EntryOverlay;
  items: LazyStoreMap<Item>;
  tags: LazyStoreSet;
  hits: LazyStoreCache<number>;
  audit: LazyStoreLog<number>;
}

function loadLazy(s: InMemoryStorage): Lazy {
  const overlay = new EntryOverlay(s as never);
  return {
    overlay,
    items: new LazyStoreMap<Item>(overlay, "items", { read: (e: { v: Item }) => e.v, write: (v, i) => ({ i, v }) }, [BY]),
    tags: new LazyStoreSet(overlay, "tags"),
    hits: new LazyStoreCache<number>(overlay, "hits", {
      read: (e: { v: number; exp: number }) => ({ v: e.v, exp: e.exp }),
      write: (c: CacheEntry<number>, i) => ({ v: c.v, exp: c.exp, i }),
      expiryOf: (e: { exp: number }) => e.exp,
    }),
    audit: new LazyStoreLog<number>(overlay, "audit", { decode: (w: number) => w, encode: (v) => v }),
  };
}

async function flushLazy(l: Lazy): Promise<void> {
  l.items.finish();
  l.tags.finish();
  l.hits.finish();
  l.audit.finish();
  await l.overlay.flush({ state: { __layout: 2 } });
}

// The stored state with the numbering that two writers may legitimately differ in taken out:
// each entry's insertion sequence becomes its rank within its field (the order is what is
// frozen), a collection's next-sequence counter is dropped (it only has to exceed every
// sequence), and a `Log` key's time and counter are dropped (its keys sort in append order, and
// the time component is only a clamp that an upper-bounding `skew` covers; see `checkSkew`).
async function dump(s: InMemoryStorage): Promise<string> {
  const all = [...(await s.list())].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const rank = new Map<string, number>();
  const byField = new Map<string, Array<[string, number]>>();
  for (const [k, v] of all) {
    if (k.startsWith("m/") || k.startsWith("s/")) {
      const field = k.slice(0, k.indexOf("/", 2));
      byField.set(field, [...(byField.get(field) ?? []), [k, (v as { i: number }).i]]);
    }
  }
  for (const list of byField.values()) list.sort((a, b) => a[1] - b[1]).forEach(([k], r) => rank.set(k, r));
  const out = all.map(([k, v]) => {
    if (k.startsWith("m/") || k.startsWith("s/")) return [k, { ...(v as object), i: rank.get(k) }];
    if (k.startsWith("c/")) {
      const { seq, skew, ...rest } = v as { seq: number; skew?: number };
      void seq;
      void skew;
      return [k, rest];
    }
    if (k.startsWith("l/")) return [k.slice(0, k.indexOf("/", 2) + 1), v];
    return [k, v];
  });
  return JSON.stringify(out);
}

// A `Log`'s `skew` must cover every entry: its key's time is at most `skew` above the entry's `t`.
async function checkSkew(s: InMemoryStorage): Promise<void> {
  const all = await s.list();
  const skew = ((all.get("c/audit") as { skew?: number } | undefined)?.skew ?? 0);
  for (const [k, v] of all) {
    if (!k.startsWith("l/audit/")) continue;
    const keyTime = Number(k.slice("l/audit/".length, "l/audit/".length + 16));
    assert.ok(keyTime >= (v as { t: number }).t, `${k} is keyed before its entry`);
    assert.ok(keyTime - (v as { t: number }).t <= skew, `${k} is keyed ${keyTime - (v as { t: number }).t} above its entry, past skew ${skew}`);
  }
}

// The next-sequence counter must stay above every sequence a collection holds.
async function checkCounters(s: InMemoryStorage): Promise<void> {
  const all = await s.list();
  for (const [k, v] of all) {
    if (!k.startsWith("c/")) continue;
    const field = k.slice(2);
    const seq = (v as { seq: number }).seq;
    for (const [ek, ev] of all) {
      if ((ek.startsWith(`m/${field}/`) || ek.startsWith(`s/${field}/`)) && (ev as { i: number }).i >= seq) {
        assert.fail(`${ek} has i ${(ev as { i: number }).i} at or above ${k}.seq ${seq}`);
      }
    }
  }
}

// One random handler's operations, applied to both and compared as it goes.
async function handler(r: () => number, e: Eager, l: Lazy, clock: { now: number }): Promise<void> {
  const pick = <T>(xs: readonly T[]): T => xs[Math.floor(r() * xs.length)]!;
  for (let step = 0; step < 14; step++) {
    const op = Math.floor(r() * 12);
    const k = pick(KEYS);
    if (op === 0 || op === 1) {
      const v = { team: pick(TEAMS), n: Math.floor(r() * 100) };
      e.items.put(k, v);
      await l.items.put(k, v);
    } else if (op === 2) {
      assert.equal(await l.items.remove(k), e.items.remove(k), `remove ${k}`);
    } else if (op === 3) {
      assert.deepEqual(await l.items.get(k), e.items.get(k), `get ${k}`);
      assert.equal(await l.items.has(k), e.items.has(k), `has ${k}`);
    } else if (op === 4) {
      e.tags.add(k);
      await l.tags.add(k);
    } else if (op === 5) {
      assert.equal(await l.tags.remove(k), e.tags.remove(k), `tag remove ${k}`);
    } else if (op === 6) {
      const c = { v: Math.floor(r() * 100), exp: clock.now + Math.floor(r() * 50) - 10 };
      e.hits.put(k, c);
      await l.hits.put(k, c);
    } else if (op === 7) {
      assert.equal(await l.hits.remove(k), e.hits.remove(k), `cache remove ${k}`);
    } else if (op === 8) {
      clock.now += Math.floor(r() * 10) - (r() < 0.2 ? 6 : 0); // sometimes backwards
      const v = Math.floor(r() * 1000);
      e.audit.append(v, clock.now);
      await l.audit.append(v, clock.now);
    } else if (op === 9) {
      const cutoff = clock.now - Math.floor(r() * 30);
      e.audit.prune(cutoff);
      await l.audit.prune(cutoff);
    } else if (op === 10) {
      for (const t of TEAMS) assert.deepEqual(await l.items.byIndex("by", t), e.items.byIndex("by", t), `byIndex ${t}`);
    } else {
      assert.equal(await l.hits.liveSize(clock.now), e.hits.liveSize(clock.now), "liveSize");
    }
  }
  // Every read agrees at the end of the handler.
  assert.deepEqual(await l.items.keys(), e.items.keys(), "keys");
  assert.deepEqual(await l.items.values(), e.items.values(), "values");
  assert.deepEqual(await l.items.entries(), e.items.entries(), "entries");
  assert.equal(await l.items.size(), e.items.size(), "size");
  assert.deepEqual(await l.tags.keys(), e.tags.keys(), "set keys");
  assert.equal(await l.tags.size(), e.tags.size());
  assert.deepEqual(await l.hits.entries(), e.hits.entries(), "cache entries");
  assert.equal(await l.hits.size(), e.hits.size());
  assert.deepEqual(await l.audit.entries(), e.audit.entries(), "log entries");
  assert.deepEqual(await l.audit.recent(3), e.audit.recent(3), "recent");
  assert.deepEqual(await l.audit.reversed(), e.audit.reversed(), "reversed");
  const a = clock.now - 20;
  const b = clock.now - 5;
  assert.deepEqual(await l.audit.since(a), e.audit.since(a), "since");
  assert.deepEqual(await l.audit.before(b), e.audit.before(b), "before");
  assert.deepEqual(await l.audit.between(a, b), e.audit.between(a, b), "between");
  assert.equal(await l.audit.size(), e.audit.size());
}

for (let seed = 1; seed <= 25; seed++) {
  test(`lazy handles match the in-memory ones and leave the same stored state (seed ${seed})`, async () => {
    const r = rng(seed);
    const sEager = new InMemoryStorage();
    const sLazy = new InMemoryStorage();
    const selfE = {};
    const clock = { now: 1000 };
    for (let h = 0; h < 6; h++) {
      const e = await loadEager(selfE, sEager);
      // The lazy side reads the *eager* side's bytes before this handler, so a handler's reads
      // run over exactly what the writer produced; both then write and must stay identical.
      const l = loadLazy(sLazy);
      await handler(r, e, l, clock);
      await commitEager(selfE, sEager, e);
      await flushLazy(l);
      assert.equal(await dump(sLazy), await dump(sEager), `stored state after handler ${h}`);
      await checkCounters(sLazy);
      await checkCounters(sEager);
      await checkSkew(sLazy);
      await checkSkew(sEager);
    }
  });
}

test("a one-entry put and an indexed read touch a bounded number of keys, whatever the size", async () => {
  for (const n of [10, 1000]) {
    const s = new InMemoryStorage();
    const e: Eager = { items: new StoreMap<Item>([BY]), tags: new StoreSet(), hits: new StoreCache<number>(), audit: new StoreLog<number>() };
    for (let i = 0; i < n; i++) e.items.put(`k${i}`, { team: TEAMS[i % 3]!, n: i });
    await commitEager({}, s, e);
    s.resetStats();
    const l = loadLazy(s);
    await l.items.put("k0", { team: "red", n: 999 });
    await flushLazy(l);
    assert.ok(s.stats.keysRead <= 3, `put read ${s.stats.keysRead} keys at n=${n}`);
    assert.ok(s.stats.keysWritten <= 3, `put wrote ${s.stats.keysWritten} keys at n=${n}`);
    s.resetStats();
    const q = loadLazy(s);
    await q.items.byIndex("by", "blue");
    assert.ok(s.stats.keysRead <= n / 3 * 2 + 2, `indexed read read ${s.stats.keysRead} keys at n=${n}`);
    s.resetStats();
    const g = loadLazy(s);
    assert.ok((await g.items.get("k1")) !== undefined);
    assert.ok(s.stats.keysRead <= 1, `get read ${s.stats.keysRead} keys at n=${n}`);
  }
});

test("cache reaping through the handle deletes what the commit-time reaper would", async () => {
  const sE = new InMemoryStorage();
  const sL = new InMemoryStorage();
  const selfE = {};
  const eager = await loadEager(selfE, sE);
  const lazy = loadLazy(sL);
  for (let i = 0; i < 80; i++) {
    const c = { v: i, exp: 10 + i };
    eager.hits.put(`k${i}`, c);
    await lazy.hits.put(`k${i}`, c);
  }
  await commitEager(selfE, sE, eager);
  await flushLazy(lazy);
  assert.equal(await dump(sL), await dump(sE));
  // A later handler reads the clock at 60: the commit reaps the 50 oldest entries by expiry.
  const e2 = await loadEager(selfE, sE);
  const state = {};
  noteClock(state, 60);
  await commitEager(selfE, sE, e2, state);
  const l2 = loadLazy(sL);
  assert.equal(await l2.hits.reap(60, 64), 51);
  await flushLazy(l2);
  assert.equal(await dump(sL), await dump(sE));
  assert.ok(expiredRange("hits", 60).prefix !== undefined);
});
