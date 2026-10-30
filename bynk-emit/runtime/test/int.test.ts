import { test } from "node:test";
import assert from "node:assert/strict";
import {
  __bynkIntDiv,
  __bynkIntDivFloor,
  __bynkIntMod,
  __bynkIntPow,
  __bynkIntRem,
  __bynkToInt,
} from "../src/int.ts";

test("#1657 __bynkIntDiv truncates toward zero and traps on zero", () => {
  assert.equal(__bynkIntDiv(7, 2), 3);
  assert.equal(__bynkIntDiv(-7, 2), -3);
  assert.throws(() => __bynkIntDiv(5, 0), /Int division by zero/);
  assert.throws(() => __bynkIntDiv(0, 0), /Int division by zero/);
});

test("#1657 __bynkToInt passes a safe integer and traps otherwise", () => {
  assert.equal(__bynkToInt(Math.round(2.5), "round"), 3);
  assert.throws(() => __bynkToInt(Math.round(Infinity), "round"), /Float.round: result is not a safe Int/);
  assert.throws(() => __bynkToInt(Math.floor(NaN), "floor"), /not a safe Int/);
  assert.throws(() => __bynkToInt(Math.trunc(1e300), "truncate"), /not a safe Int/);
});

// Standard-library K6b (#2598).
test("rem takes the dividend's sign and pairs with truncating /", () => {
  for (const [a, b] of [[7, 2], [-7, 2], [7, -2], [-7, -2], [6, 3], [-6, 3]]) {
    const r = __bynkIntRem(a, b);
    assert.equal(b * __bynkIntDiv(a, b) + r, a, `${a} rem ${b}`);
    assert.ok(r === 0 || Math.sign(r) === Math.sign(a), `${a} rem ${b} = ${r}`);
    assert.ok(!Object.is(r, -0));
  }
  assert.equal(__bynkIntRem(-7, 2), -1);
});

test("div floors and mod takes the divisor's sign, pairing with each other", () => {
  assert.equal(__bynkIntDivFloor(-7, 2), -4);
  assert.equal(__bynkIntMod(-7, 2), 1);
  assert.equal(__bynkIntMod(7, -2), -1);
  for (const [a, b] of [[7, 2], [-7, 2], [7, -2], [-7, -2], [6, 3], [-6, 3], [0, 5]]) {
    const d = __bynkIntDivFloor(a, b);
    const m = __bynkIntMod(a, b);
    assert.equal(b * d + m, a, `${a} div/mod ${b}`);
    assert.ok(!Object.is(m, -0));
  }
  for (let x = -20; x <= 20; x++) {
    const m = __bynkIntMod(x, 7);
    assert.ok(m >= 0 && m < 7, `${x} mod 7 = ${m}`);
  }
});

test("div, rem and mod fault on a zero divisor", () => {
  assert.throws(() => __bynkIntRem(1, 0), /Int\.rem: division by zero/);
  assert.throws(() => __bynkIntDivFloor(1, 0), /Int\.div: division by zero/);
  assert.throws(() => __bynkIntMod(1, 0), /Int\.mod: division by zero/);
});

test("pow is exact to the safe range and faults past it or on a negative exponent", () => {
  assert.equal(__bynkIntPow(0, 0), 1);
  assert.equal(__bynkIntPow(2, 10), 1024);
  assert.equal(__bynkIntPow(-3, 3), -27);
  assert.equal(__bynkIntPow(2, 52), 4503599627370496);
  assert.equal(__bynkIntPow(3, 33), 5559060566555523);
  assert.throws(() => __bynkIntPow(2, 53), /past the safe Int range/);
  assert.throws(() => __bynkIntPow(10, 16), /past the safe Int range/);
  assert.throws(() => __bynkIntPow(2, -1), /negative exponent/);
});
