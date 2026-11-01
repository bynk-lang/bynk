// #2684: composite keys for a value `Map`/`Set` (constrained-generics slice 2).
import { test } from "node:test";
import assert from "node:assert/strict";
import { None, Some } from "../src/result.ts";
import { __bynkEq } from "../src/equality.ts";
import {
  __bynkKeyEncode,
  __BynkKeyMap,
  __BynkKeySet,
  __bynkMapInsert,
  __bynkSetInsert,
  __bynkSetRemove,
  __bynkSetOf,
  __bynkDistinct,
  __bynkDistinctBy,
  __bynkGroupBy,
  __bynkJoinOn,
  __bynkLeftJoin,
} from "../src/keys.ts";

test("the encoding ignores field order and quotes nested strings", () => {
  assert.equal(
    __bynkKeyEncode({ region: "eu", n: 7 }),
    __bynkKeyEncode({ n: 7, region: "eu" }),
  );
  assert.notEqual(__bynkKeyEncode({ tag: "A", v: "1" }), __bynkKeyEncode({ tag: "A", v: 1 }));
  // A string and a number under one sum's payload slot stay distinct.
  assert.notEqual(__bynkKeyEncode({ tag: "S", value: "1" }), __bynkKeyEncode({ tag: "N", value: 1 }));
});

test("nested sums and options do not collide", () => {
  assert.notEqual(__bynkKeyEncode(Some("x")), __bynkKeyEncode("x"));
  assert.notEqual(__bynkKeyEncode(Some(Some(1))), __bynkKeyEncode(Some(1)));
  assert.notEqual(__bynkKeyEncode(None), __bynkKeyEncode(Some(None)));
  assert.notEqual(__bynkKeyEncode({ tag: "A", x: { tag: "B" } }), __bynkKeyEncode({ tag: "B", x: { tag: "A" } }));
});

test("Bytes encode as hex: [1, 0] is not [10], [0x1, 0x0] is not [0x10]", () => {
  assert.notEqual(__bynkKeyEncode([1, 0]), __bynkKeyEncode([10]));
  assert.notEqual(__bynkKeyEncode(Uint8Array.of(1, 0)), __bynkKeyEncode(Uint8Array.of(16)));
  assert.equal(__bynkKeyEncode(Uint8Array.of(1, 0)), __bynkKeyEncode(Uint8Array.of(1, 0)));
  assert.notEqual(__bynkKeyEncode(Uint8Array.of(1, 0)), __bynkKeyEncode([1, 0]));
});

test("-0 and 0 are one key", () => {
  assert.equal(__bynkKeyEncode(-0), __bynkKeyEncode(0));
  assert.equal(__bynkKeyEncode({ a: -0 }), __bynkKeyEncode({ a: 0 }));
});

test("a keyed map looks up, replaces and removes by value, keeping real keys in order", () => {
  const a = { region: "eu", n: 7 };
  const m = new __BynkKeyMap<{ region: string; n: number }, number>();
  m.set(a, 1).set({ region: "us", n: 1 }, 2).set({ n: 7, region: "eu" }, 3);
  assert.equal(m.size, 2);
  assert.equal(m.get({ region: "eu", n: 7 }), 3);
  assert.equal(m.has({ region: "us", n: 1 }), true);
  assert.deepEqual([...m.keys()], [a, { region: "us", n: 1 }]);
  assert.equal(m.keys().next().value, a);
  assert.equal(m.delete({ n: 1, region: "us" }), true);
  assert.equal(m.delete({ n: 1, region: "us" }), false);
  assert.deepEqual([...m.values()], [3]);
});

test("a keyed set dedups by value and preserves first position", () => {
  const s = new __BynkKeySet<{ tag: string }>([{ tag: "Open" }, { tag: "Shipped" }, { tag: "Open" }]);
  assert.equal(s.size, 2);
  assert.deepEqual([...s], [{ tag: "Open" }, { tag: "Shipped" }]);
  assert.equal(s.has({ tag: "Shipped" }), true);
});

test("insert promotes a plain collection on its first composite key, and copies", () => {
  const empty: ReadonlyMap<unknown, number> = new Map();
  const one = __bynkMapInsert(empty, { a: 1 }, 1);
  assert.ok(one instanceof __BynkKeyMap);
  assert.equal(empty.size, 0);
  const two = __bynkMapInsert(one, { a: 1 }, 2);
  assert.equal(two.size, 1);
  assert.equal(one.get({ a: 1 }), 1);
  assert.equal(two.get({ a: 1 }), 2);
  // A primitive key keeps the collection plain.
  const plain = __bynkMapInsert(new Map<string, number>(), "k", 1);
  assert.equal(plain instanceof __BynkKeyMap, false);
  const s = __bynkSetInsert(new Set<unknown>(), { a: 1 });
  assert.ok(s instanceof __BynkKeySet);
  assert.equal(__bynkSetInsert(s, { a: 1 }).size, 1);
  assert.equal(__bynkSetRemove(s, { a: 1 }).size, 0);
  assert.equal(s.size, 1);
  assert.equal(__bynkSetOf([1, 2, 1]) instanceof __BynkKeySet, false);
  assert.equal(__bynkSetOf([{ a: 1 }, { a: 1 }]).size, 1);
});

test("copying a keyed collection is independent of the original", () => {
  const m = new __BynkKeyMap<{ a: number }, number>([[{ a: 1 }, 1]]);
  const c = new __BynkKeyMap(m);
  c.set({ a: 2 }, 2).set({ a: 1 }, 9);
  assert.equal(m.size, 1);
  assert.equal(m.get({ a: 1 }), 1);
  assert.equal(c.get({ a: 1 }), 9);
  assert.equal(c.has({ a: 2 }), true);
  const s = new __BynkKeySet<{ a: number }>([{ a: 1 }]);
  const t = new __BynkKeySet(s);
  t.add({ a: 2 });
  t.delete({ a: 1 });
  assert.equal(s.has({ a: 1 }), true);
  assert.equal(s.has({ a: 2 }), false);
  assert.equal(t.size, 1);
});

test("__bynkEq compares keyed collections structurally", () => {
  const a = new __BynkKeySet([{ x: 1 }, { x: 2 }]);
  const b = new __BynkKeySet([{ x: 2 }, { x: 1 }]);
  assert.equal(__bynkEq(a, b), true);
  assert.equal(__bynkEq(a, new __BynkKeySet([{ x: 1 }])), false);
  // A keyed set against a plain-built empty one.
  assert.equal(__bynkEq(new __BynkKeySet<number>(), new Set<number>()), true);
  const m1 = new __BynkKeyMap([[{ k: "a" }, 1]]);
  const m2 = new __BynkKeyMap([[{ k: "a" }, 1]]);
  assert.equal(__bynkEq(m1, m2), true);
  assert.equal(__bynkEq(m1, new __BynkKeyMap([[{ k: "a" }, 2]])), false);
});

test("distinct, distinctBy, groupBy and joins use the encoding", () => {
  assert.deepEqual(__bynkDistinct([{ a: 1 }, { a: 2 }, { a: 1 }]), [{ a: 1 }, { a: 2 }]);
  assert.deepEqual(
    __bynkDistinctBy([{ id: { n: 1 }, v: "x" }, { id: { n: 1 }, v: "y" }], (r) => r.id),
    [{ id: { n: 1 }, v: "x" }],
  );
  const g = __bynkGroupBy(
    [{ k: { r: "a" }, v: 1 }, { k: { r: "b" }, v: 2 }, { k: { r: "a" }, v: 3 }],
    (x) => x.k,
    (k, rows) => [k.r, rows.map((r) => r.v)],
  );
  assert.deepEqual(g, [["a", [1, 3]], ["b", [2]]]);
  const l = [{ id: { n: 1 } }, { id: { n: 2 } }];
  const r = [{ id: { n: 2 }, w: "two" }];
  assert.deepEqual(
    __bynkJoinOn(l, r, (x) => x.id, (y) => y.id, (x, y) => [x.id.n, y.w]),
    [[2, "two"]],
  );
  assert.deepEqual(
    __bynkLeftJoin(l, r, (x) => x.id, (y) => y.id, (x, y) => [x.id.n, y.tag === "Some" ? y.value.w : null]),
    [[1, null], [2, "two"]],
  );
});
