import { test } from "node:test";
import assert from "node:assert/strict";
import { EntryOverlay } from "../src/entries.ts";
import { LazyStoreLog, LazyStoreMap, SCAN_PAGE } from "../src/lazy.ts";
import type { Indexer } from "../src/store.ts";
import { None, Some } from "../src/result.ts";
import { scanAll, scanAny, scanFilter, scanFirst, scanFirstOrElse, scanFlatMap, scanMap, scanSkip, scanTake } from "../src/scan.ts";
import { InMemoryStorage } from "../src/storage.ts";
import type { DurableObjectListOptions } from "../src/storage.ts";

// The lazy scans (#4036): the same answers as reading every value, and, where the answer is known
// early, the pages after it are never read.

class CountingStorage extends InMemoryStorage {
  lists = 0;
  override async list<T>(options?: DurableObjectListOptions): Promise<Map<string, T>> {
    this.lists += 1;
    return super.list<T>(options);
  }
}

const N = SCAN_PAGE * 3 + 11;

async function drain<T>(source: AsyncIterable<T>): Promise<T[]> {
  const out: T[] = [];
  for await (const x of source) out.push(x);
  return out;
}

async function seeded(): Promise<{ s: CountingStorage; items: LazyStoreMap<number>; audit: LazyStoreLog<number> }> {
  const s = new CountingStorage();
  const overlay = new EntryOverlay(s as never);
  const items = new LazyStoreMap<number>(overlay, "items", { read: (e: { v: number }) => e.v, write: (v, i) => ({ i, v }) }, []);
  const audit = new LazyStoreLog<number>(overlay, "audit", { decode: (w: number) => w, encode: (v) => v });
  // Map keys in the reverse of their insertion order, so key order and scan order differ.
  for (let i = 0; i < N; i += 1) await items.put(String(N - i).padStart(6, "0"), i);
  for (let i = 0; i < N; i += 1) await audit.append(i, 1000 + i);
  items.finish();
  audit.finish();
  await overlay.flush({ state: { __layout: 2 } });
  const read = new EntryOverlay(s as never);
  return {
    s,
    items: new LazyStoreMap<number>(read, "items", { read: (e: { v: number }) => e.v, write: (v, i) => ({ i, v }) }, []),
    audit: new LazyStoreLog<number>(read, "audit", { decode: (w: number) => w, encode: (v) => v }),
  };
}

test("a Log scan yields the values oldest first, and any stops at the first page that answers", async () => {
  const { s, audit } = await seeded();
  const all = await audit.values();
  assert.equal(all.length, N);
  assert.deepEqual(await drain(audit.lazyValues()), [...all]);
  s.lists = 0;
  assert.equal(await scanAny(audit.lazyValues(), (x) => x === 3), true);
  assert.equal(s.lists, 1, "a match on the first page reads one page");
  s.lists = 0;
  assert.equal(await scanAny(audit.lazyValues(), (x) => x === N + 5), false);
  assert.equal(s.lists, 4, "no match reads every page");
});

test("all stops at the first counter-example", async () => {
  const { s, audit } = await seeded();
  s.lists = 0;
  assert.equal(await scanAll(audit.lazyValues(), (x) => x < 5), false);
  assert.equal(s.lists, 1);
  s.lists = 0;
  assert.equal(await scanAll(audit.lazyValues(), (x) => x >= 0), true);
  assert.equal(s.lists, 4);
});

test("first, firstOrElse and take on a Log read only the pages they need", async () => {
  const { s, audit } = await seeded();
  s.lists = 0;
  assert.deepEqual(await scanFirst(scanFilter(audit.lazyValues(), (x) => x % 2 === 1)), Some(1));
  assert.equal(s.lists, 1);
  s.lists = 0;
  assert.equal(await scanFirstOrElse(scanFilter(audit.lazyValues(), (x) => x > SCAN_PAGE + 3), -1), SCAN_PAGE + 4);
  assert.equal(s.lists, 2, "the match is on the second page");
  s.lists = 0;
  assert.deepEqual(await scanTake(audit.lazyValues(), 3), [0, 1, 2]);
  assert.equal(s.lists, 1);
  s.lists = 0;
  assert.deepEqual(await scanTake(scanSkip(audit.lazyValues(), SCAN_PAGE - 1), 3), [SCAN_PAGE - 1, SCAN_PAGE, SCAN_PAGE + 1]);
  assert.equal(s.lists, 2, "a window across a page boundary reads both pages");
  assert.deepEqual(await scanFirst(scanFilter(audit.lazyValues(), (x) => x < 0)), None);
  assert.deepEqual(await scanFirstOrElse(scanFilter(audit.lazyValues(), (x) => x < 0), -1), -1);
  assert.deepEqual(await scanTake(audit.lazyValues(), 0), []);
  assert.deepEqual(await scanTake(audit.lazyValues(), N + 10), [...(await audit.values())]);
});

test("map and flatMap stages agree with the arrays", async () => {
  const { audit } = await seeded();
  const all = [...(await audit.values())];
  assert.deepEqual(await scanTake(scanMap(audit.lazyValues(), (x) => x * 2), 5), all.slice(0, 5).map((x) => x * 2));
  assert.deepEqual(
    await scanTake(scanFlatMap(audit.lazyValues(), (x) => () => [x, x]), 5),
    all.slice(0, 3).flatMap((x) => [x, x]).slice(0, 5),
  );
});

test("a Map scan reads every entry in key order, and any gives the same answer as the array", async () => {
  const { s, items } = await seeded();
  const all = [...(await items.values())];
  assert.equal(all.length, N);
  const lazy = await drain(items.lazyValues());
  assert.deepEqual([...lazy].sort((a, b) => a - b), [...all].sort((a, b) => a - b), "the same values");
  assert.notDeepEqual(lazy, all, "key order is not insertion order");
  for (const p of [(x: number) => x === 0, (x: number) => x === N - 1, (x: number) => x < 0, (x: number) => x >= 0]) {
    assert.equal(await scanAny(items.lazyValues(), p), all.some(p));
    assert.equal(await scanAll(items.lazyValues(), p), all.every(p));
  }
  s.lists = 0;
  assert.equal(await scanAny(items.lazyValues(), (x) => x === N - 1), true);
  assert.equal(s.lists, 1, "the last-inserted value has the smallest key: the first page");
});

// #4300: an equality filter on an `@indexed` field reads that value's posting list, and `any` and `all`
// over it read the list a page at a time.
test("lazyByIndex yields the posting list's values, and any stops at the first page that answers", async () => {
  interface Row {
    team: string;
    n: number;
  }
  const BY: Indexer<Row> = { index: "by", of: (r) => r.team };
  const s = new CountingStorage();
  const overlay = new EntryOverlay(s as never);
  const codec = { read: (e: { v: Row }) => e.v, write: (v: Row, i: number) => ({ i, v }) };
  const write = new LazyStoreMap<Row>(overlay, "rows", codec, [BY]);
  const total = SCAN_PAGE * 2 + 5;
  for (let i = 0; i < total; i += 1) await write.put(String(i).padStart(6, "0"), { team: i % 3 === 0 ? "red" : "blue", n: i });
  write.finish();
  await overlay.flush({ state: { __layout: 2 } });
  const rows = new LazyStoreMap<Row>(new EntryOverlay(s as never), "rows", codec, [BY]);

  const red = await rows.byIndex("by", "red");
  const lazy = await drain(rows.lazyByIndex("by", "red"));
  assert.deepEqual(lazy.map((r) => r.n).sort((a, b) => a - b), red.map((r) => r.n).sort((a, b) => a - b), "the same values");
  assert.equal(lazy.every((r) => r.team === "red"), true);

  s.lists = 0;
  assert.equal(await scanAny(rows.lazyByIndex("by", "red"), (r) => r.n === 0), true);
  assert.equal(s.lists, 1, "a match on the first page of postings reads one page");
  s.lists = 0;
  assert.equal(await scanAll(rows.lazyByIndex("by", "red"), (r) => r.n % 3 !== 0), false);
  assert.equal(s.lists, 1, "the first counter-example is on the first page");
  s.lists = 0;
  assert.equal(await scanAny(rows.lazyByIndex("by", "red"), (r) => r.n < 0), false);
  assert.equal(s.lists, 1 + Math.floor(red.length / SCAN_PAGE), "no match reads every page of that value's postings");
  assert.deepEqual(await drain(rows.lazyByIndex("by", "none")), []);
  await assert.rejects(() => drain(rows.lazyByIndex("nope", "red")), /no index "nope"/);
});
