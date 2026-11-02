// #2660: the comparators behind `Ord` (constrained-generics slice 1).
import { test } from "node:test";
import assert from "node:assert/strict";
import { Some, None } from "../src/result.ts";
import {
  __bynkCmpInt,
  __bynkCmpString,
  __bynkCmpFloat,
  __bynkCmpBytes,
  __bynkCmpOption,
  __bynkCmpList,
  __bynkCmpRecord,
  __bynkCmpSum,
  __bynkCmpFix,
} from "../src/compare.ts";

const sign = (n: number): number => (n < 0 ? -1 : n > 0 ? 1 : 0);

test("Int and String compare in numeric and code-unit order", () => {
  assert.equal(sign(__bynkCmpInt(1, 2)), -1);
  assert.equal(sign(__bynkCmpInt(2, 1)), 1);
  assert.equal(sign(__bynkCmpInt(2, 2)), 0);
  assert.equal(sign(__bynkCmpString("a", "b")), -1);
  // UTF-16 code units: U+FFFF sorts after a surrogate pair's lead unit.
  assert.equal(sign(__bynkCmpString("\u{1F600}", "￿")), -1);
});

test("Float is a total order: NaN last and equal to itself, -0 equal to 0", () => {
  assert.equal(__bynkCmpFloat(NaN, NaN), 0);
  assert.equal(sign(__bynkCmpFloat(NaN, 1)), 1);
  assert.equal(sign(__bynkCmpFloat(1, NaN)), -1);
  assert.equal(sign(__bynkCmpFloat(NaN, Infinity)), 1);
  assert.equal(__bynkCmpFloat(-0, 0), 0);
  assert.equal(sign(__bynkCmpFloat(-1, 0)), -1);
  assert.deepEqual([3, NaN, 1, NaN, 2].sort(__bynkCmpFloat).map(String), ["1", "2", "3", "NaN", "NaN"]);
});

test("Bytes are unsigned lexicographic", () => {
  assert.equal(sign(__bynkCmpBytes(Uint8Array.of(1), Uint8Array.of(2))), -1);
  assert.equal(sign(__bynkCmpBytes(Uint8Array.of(255), Uint8Array.of(1))), 1);
  assert.equal(sign(__bynkCmpBytes(Uint8Array.of(1), Uint8Array.of(1, 0))), -1);
});

test("Option puts None first; List is lexicographic", () => {
  const o = __bynkCmpOption(__bynkCmpInt);
  assert.equal(sign(o(None, Some(1))), -1);
  assert.equal(sign(o(Some(1), None)), 1);
  assert.equal(o(None, None), 0);
  assert.equal(sign(o(Some(1), Some(2))), -1);
  const l = __bynkCmpList(__bynkCmpInt);
  assert.equal(sign(l([1], [1, 0])), -1);
  assert.equal(sign(l([2], [1, 9])), 1);
  assert.equal(l([], []), 0);
});

test("a record compares its fields in order; a sum by variant position then payload", () => {
  const rec = __bynkCmpRecord<{ a: number; b: string }>([
    ["a", __bynkCmpInt],
    ["b", __bynkCmpString],
  ]);
  assert.equal(sign(rec({ a: 1, b: "z" }, { a: 2, b: "a" })), -1);
  assert.equal(sign(rec({ a: 1, b: "b" }, { a: 1, b: "a" })), 1);
  // Key order in the object does not matter.
  assert.equal(rec({ b: "a", a: 1 }, { a: 1, b: "a" }), 0);

  type S = { tag: "Low" } | { tag: "High" } | { tag: "Mid"; n: number };
  const sum = __bynkCmpSum<S>(["Low", "Mid", "High"], { Mid: [["n", __bynkCmpInt]] });
  assert.equal(sign(sum({ tag: "Low" }, { tag: "High" })), -1);
  assert.equal(sign(sum({ tag: "High" }, { tag: "Mid", n: 9 })), 1);
  assert.equal(sign(sum({ tag: "Mid", n: 1 }, { tag: "Mid", n: 2 })), -1);
  assert.equal(sum({ tag: "Low" }, { tag: "Low" }), 0);
});

test("__bynkCmpFix lets a recursive type name its own comparator", () => {
  type E = { tag: "Num"; n: number } | { tag: "Neg"; inner: E };
  const cmp = __bynkCmpFix<E>((self) =>
    __bynkCmpSum<E>(["Num", "Neg"], { Num: [["n", __bynkCmpInt]], Neg: [["inner", self]] }),
  );
  const num = (n: number): E => ({ tag: "Num", n });
  const neg = (inner: E): E => ({ tag: "Neg", inner });
  assert.equal(sign(cmp(num(9), neg(num(0)))), -1);
  assert.equal(sign(cmp(neg(neg(num(1))), neg(neg(num(2))))), -1);
  assert.equal(cmp(neg(num(1)), neg(num(1))), 0);
});
