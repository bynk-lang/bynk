import { test } from "node:test";
import assert from "node:assert/strict";
import { StoreCache, StoreLog, StoreMap, StoreSet, type Indexer } from "../src/store.ts";

// The in-memory store collection handles (#3254): insertion order for every key,
// including the integer-like ones a plain object enumerates by value.

test("map: integer-like and other keys enumerate in insertion order", () => {
  const m = new StoreMap<number>();
  for (const [k, v] of [["10", 1], ["2", 2], ["a", 3], ["5", 4]] as const) m.put(k, v);
  assert.deepEqual(m.keys(), ["10", "2", "a", "5"]);
  assert.deepEqual(m.values(), [1, 2, 3, 4]);
  assert.deepEqual(m.entries(), [["10", 1], ["2", 2], ["a", 3], ["5", 4]]);
  assert.equal(m.size(), 4);
});

test("map: the same keys in a plain object are NOT in insertion order (the defect)", () => {
  const o: Record<string, number> = {};
  for (const k of ["10", "2", "a", "5"]) o[k] = 0;
  assert.deepEqual(Object.keys(o), ["2", "5", "10", "a"]);
});

test("map: re-inserting a key keeps its position", () => {
  const m = new StoreMap<string>();
  m.put("10", "x");
  m.put("2", "y");
  m.put("a", "z");
  m.put("2", "y2");
  assert.deepEqual(m.keys(), ["10", "2", "a"]);
  assert.equal(m.get("2"), "y2");
});

test("map: removing a key then inserting it again moves it to the end", () => {
  const m = new StoreMap<string>();
  for (const k of ["10", "2", "a", "5"]) m.put(k, k);
  assert.equal(m.remove("2"), true);
  assert.equal(m.remove("2"), false);
  m.put("2", "again");
  assert.deepEqual(m.keys(), ["10", "a", "5", "2"]);
});

test("map: has, get and size", () => {
  const m = new StoreMap<number>();
  assert.equal(m.has("k"), false);
  assert.equal(m.get("k"), undefined);
  m.put("k", 1);
  assert.equal(m.has("k"), true);
  assert.equal(m.get("k"), 1);
  assert.equal(m.size(), 1);
  m.remove("k");
  assert.equal(m.size(), 0);
});

test("map: a key named like an Object prototype member is an ordinary key", () => {
  const m = new StoreMap<number>();
  m.put("__proto__", 1);
  m.put("constructor", 2);
  m.put("hasOwnProperty", 3);
  assert.deepEqual(m.keys(), ["__proto__", "constructor", "hasOwnProperty"]);
  assert.equal(m.get("__proto__"), 1);
  assert.equal(m.has("toString"), false);
});

test("map: from() builds a handle in the order given", () => {
  const m = StoreMap.from([["b", 1], ["10", 2], ["a", 3]]);
  assert.deepEqual(m.keys(), ["b", "10", "a"]);
});

interface Line {
  readonly order: string;
  readonly sku: string;
}

const byOrder: Indexer<Line> = { index: "order", of: (l) => l.order };

test("index: byIndex reads a posting list in insertion order", () => {
  const m = new StoreMap<Line>([byOrder]);
  m.put("l1", { order: "o1", sku: "a" });
  m.put("l2", { order: "o2", sku: "b" });
  m.put("l3", { order: "o1", sku: "c" });
  assert.deepEqual(m.byIndex("order", "o1").map((l) => l.sku), ["a", "c"]);
  assert.deepEqual(m.byIndex("order", "o2").map((l) => l.sku), ["b"]);
  assert.deepEqual(m.byIndex("order", "none"), []);
});

test("index: an overwrite that changes the indexed value moves the posting, at its insertion rank", () => {
  const m = new StoreMap<Line>([byOrder]);
  m.put("l1", { order: "o1", sku: "a" });
  m.put("l2", { order: "o2", sku: "b" });
  m.put("l3", { order: "o2", sku: "c" });
  // l1 was inserted before l2 and l3, so under o2 it comes first.
  m.put("l1", { order: "o2", sku: "a" });
  assert.deepEqual(m.byIndex("order", "o2").map((l) => l.sku), ["a", "b", "c"]);
  assert.deepEqual(m.byIndex("order", "o1"), []);
  assert.deepEqual(m.keys(), ["l1", "l2", "l3"]);
});

test("index: an overwrite that keeps the indexed value leaves the posting list alone", () => {
  const m = new StoreMap<Line>([byOrder]);
  m.put("l1", { order: "o1", sku: "a" });
  m.put("l2", { order: "o1", sku: "b" });
  m.put("l1", { order: "o1", sku: "a2" });
  assert.deepEqual(m.byIndex("order", "o1").map((l) => l.sku), ["a2", "b"]);
});

test("index: remove drops the posting, and a re-inserted key joins at the end", () => {
  const m = new StoreMap<Line>([byOrder]);
  m.put("l1", { order: "o1", sku: "a" });
  m.put("l2", { order: "o1", sku: "b" });
  m.remove("l1");
  assert.deepEqual(m.byIndex("order", "o1").map((l) => l.sku), ["b"]);
  m.put("l1", { order: "o1", sku: "a" });
  assert.deepEqual(m.byIndex("order", "o1").map((l) => l.sku), ["b", "a"]);
  m.remove("l1");
  m.remove("l2");
  assert.deepEqual(m.byIndex("order", "o1"), []);
  assert.equal(m.postingsOf("order").size, 0);
});

test("index: postingsOf lists each bucket's primary keys in insertion order", () => {
  const m = new StoreMap<Line>([byOrder]);
  m.put("10", { order: "o1", sku: "a" });
  m.put("2", { order: "o1", sku: "b" });
  assert.deepEqual([...m.postingsOf("order")], [["o1", ["10", "2"]]]);
});

test("set: members enumerate in insertion order, integer-like first included", () => {
  const s = new StoreSet();
  for (const k of ["10", "2", "a", "5"]) s.add(k);
  assert.deepEqual(s.keys(), ["10", "2", "a", "5"]);
  s.add("2");
  assert.deepEqual(s.keys(), ["10", "2", "a", "5"]);
  assert.equal(s.remove("2"), true);
  assert.equal(s.remove("2"), false);
  s.add("2");
  assert.deepEqual(s.keys(), ["10", "a", "5", "2"]);
  assert.equal(s.has("a"), true);
  assert.equal(s.has("zz"), false);
  assert.equal(s.size(), 4);
});

test("the eager handles resolve through await, as a storage-backed one would", async () => {
  const m = new StoreMap<number>();
  await m.put("a", 1);
  assert.equal(await m.get("a"), 1);
  assert.deepEqual(await m.keys(), ["a"]);
  const s = StoreSet.from(["x"]);
  assert.equal(await s.has("x"), true);
});

test("cache: liveSize counts the entries whose exp is after now", () => {
  const c = new StoreCache<string>();
  c.put("a", { v: "x", exp: 100 });
  c.put("10", { v: "y", exp: 200 });
  c.put("b", { v: "z", exp: 50 });
  assert.equal(c.liveSize(0), 3);
  assert.equal(c.liveSize(50), 2);
  assert.equal(c.liveSize(100), 1);
  assert.equal(c.liveSize(200), 0);
  // It is a Map handle: insertion order, with integer-like keys in place.
  assert.deepEqual(c.keys(), ["a", "10", "b"]);
  assert.equal(c.size(), 3);
});

test("cache: ofEntries() builds a handle in the order given", () => {
  const c = StoreCache.ofEntries([["b", { v: 1, exp: 9 }], ["10", { v: 2, exp: 9 }]]);
  assert.deepEqual(c.keys(), ["b", "10"]);
});

const stamped = (): StoreLog<string> => {
  const l = new StoreLog<string>();
  l.append("a", 10);
  l.append("b", 20);
  l.append("c", 30);
  l.append("d", 40);
  return l;
};

test("log: values and entries are in append order, size counts them", () => {
  const l = stamped();
  assert.deepEqual(l.values(), ["a", "b", "c", "d"]);
  assert.deepEqual(l.entries(), [{ t: 10, v: "a" }, { t: 20, v: "b" }, { t: 30, v: "c" }, { t: 40, v: "d" }]);
  assert.equal(l.size(), 4);
});

test("log: time windows are since (>=), before (<) and between (both ends inclusive)", () => {
  const l = stamped();
  assert.deepEqual(l.since(30), ["c", "d"]);
  assert.deepEqual(l.since(31), ["d"]);
  assert.deepEqual(l.before(30), ["a", "b"]);
  assert.deepEqual(l.before(10), []);
  assert.deepEqual(l.between(20, 30), ["b", "c"]);
  assert.deepEqual(l.between(21, 29), []);
});

test("log: recent is the n newest, newest first; reversed is all, newest first", () => {
  const l = stamped();
  assert.deepEqual(l.recent(2), ["d", "c"]);
  assert.deepEqual(l.recent(0), []);
  assert.deepEqual(l.recent(-3), []);
  assert.deepEqual(l.recent(99), ["d", "c", "b", "a"]);
  assert.deepEqual(l.reversed(), ["d", "c", "b", "a"]);
});

test("log: prune drops every entry before the cutoff, wherever it sits", () => {
  const l = new StoreLog<string>();
  l.append("a", 10);
  l.append("late", 5); // appended after a, but stamped earlier (a clock that stepped back)
  l.append("b", 20);
  l.prune(10);
  assert.deepEqual(l.values(), ["a", "b"]);
  l.prune(100);
  assert.equal(l.size(), 0);
});

test("log: from() takes stamped entries as they are stored, and entries() is a copy", () => {
  const l = StoreLog.from([{ t: 1, v: "x" }, { t: 2, v: "y" }]);
  assert.deepEqual(l.values(), ["x", "y"]);
  (l.entries() as Array<unknown>).pop();
  assert.equal(l.size(), 2);
});
