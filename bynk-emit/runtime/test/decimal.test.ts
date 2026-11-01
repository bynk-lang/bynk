import { test } from "node:test";
import assert from "node:assert/strict";
import {
  __bynkDecimal,
  __bynkDecimalAdd,
  __bynkDecimalSub,
  __bynkDecimalMul,
  __bynkDecimalCmp,
  __bynkDecimalEqual,
  __bynkDecimalRound,
  __bynkDecimalDiv,
  __bynkDecimalToString,
  __bynkDecimalParse,
  __bynkDecimalToInt,
  __bynkDecimalKey,
  __bynkDecimalFromInt,
  __bynkDecimalShow,
  __bynkDecimalParseWire,
  __BYNK_DECIMAL_MAX_SCALE,
  type __BynkDecimalValue,
  type __BynkRoundingMode,
} from "../src/decimal.ts";
import { __bynkEq } from "../src/equality.ts";
import { __bynkKeyEncode } from "../src/keys.ts";

const d = (s: string): __BynkDecimalValue => {
  const p = __bynkDecimalParse(s);
  assert.equal(p.tag, "Some", s);
  return (p as { value: __BynkDecimalValue }).value;
};
const str = __bynkDecimalToString;

test("exact arithmetic: 1.10 + 2.20 == 3.30", () => {
  const sum = __bynkDecimalAdd(d("1.10"), d("2.20"));
  assert.equal(str(sum), "3.30");
  assert.equal(__bynkDecimalEqual(sum, d("3.30")), true);
  assert.equal(str(__bynkDecimalSub(d("1.00"), d("0.01"))), "0.99");
  assert.equal(str(__bynkDecimalMul(d("1.5"), d("-2.50"))), "-3.750");
});

test("equality and order are by value, scale is kept", () => {
  assert.equal(__bynkDecimalEqual(d("1.20"), d("1.2")), true);
  assert.equal(__bynkDecimalCmp(d("-1"), d("0.5")), -1);
  assert.equal(__bynkDecimalCmp(d("-0.50"), d("-0.5")), 0);
  assert.equal(__bynkDecimalCmp(d("10"), d("9.99")), 1);
  assert.equal(str(d("1.20")), "1.20");
  assert.equal(__bynkEq(d("1.20"), d("1.2")), true);
  assert.equal(__bynkEq({ a: d("2.0") }, { a: d("2") }), true);
  assert.equal(__bynkEq(d("2"), d("3")), false);
  assert.equal(__bynkKeyEncode(d("1.20")), __bynkKeyEncode(d("1.2")));
  assert.equal(__bynkDecimalKey(d("100.00")), "100");
});

const MODES: __BynkRoundingMode[] = ["Down", "Up", "Floor", "Ceiling", "HalfUp", "HalfDown", "HalfEven"];
const roundAll = (s: string, scale: number): string =>
  MODES.map((m) => str(__bynkDecimalRound(d(s), scale, m))).join(" ");

test("round honours each mode", () => {
  //                          Down  Up    Floor Ceil  HalfUp HalfDn HalfEv
  assert.equal(roundAll("2.5", 0), "2 3 2 3 3 2 2");
  assert.equal(roundAll("3.5", 0), "3 4 3 4 4 3 4");
  assert.equal(roundAll("-2.5", 0), "-2 -3 -3 -2 -3 -2 -2");
  assert.equal(roundAll("-3.5", 0), "-3 -4 -4 -3 -4 -3 -4");
  assert.equal(roundAll("2.51", 0), "2 3 2 3 3 3 3");
  assert.equal(roundAll("2.49", 0), "2 3 2 3 2 2 2");
  assert.equal(roundAll("-2.49", 0), "-2 -3 -3 -2 -2 -2 -2");
  assert.equal(roundAll("1.005", 2), "1.00 1.01 1.00 1.01 1.01 1.00 1.00");
  assert.equal(roundAll("7", 0), "7 7 7 7 7 7 7");
  // A larger scale pads, never rounds.
  assert.equal(str(__bynkDecimalRound(d("1.5"), 3, "HalfEven")), "1.500");
  assert.throws(() => __bynkDecimalRound(d("1"), -1, "Down"));
});

test("div states a scale and a mode", () => {
  assert.equal(str(__bynkDecimalDiv(d("1"), d("3"), 4, "HalfEven")), "0.3333");
  assert.equal(str(__bynkDecimalDiv(d("2"), d("3"), 4, "HalfEven")), "0.6667");
  assert.equal(str(__bynkDecimalDiv(d("2"), d("3"), 4, "Down")), "0.6666");
  assert.equal(str(__bynkDecimalDiv(d("-1"), d("3"), 2, "Floor")), "-0.34");
  assert.equal(str(__bynkDecimalDiv(d("-1"), d("3"), 2, "Ceiling")), "-0.33");
  assert.equal(str(__bynkDecimalDiv(d("10.00"), d("4"), 0, "HalfEven")), "2");
  assert.equal(str(__bynkDecimalDiv(d("10.00"), d("4"), 0, "HalfUp")), "3");
  assert.equal(str(__bynkDecimalDiv(d("1"), d("-0.5"), 1, "Down")), "-2.0");
  assert.throws(() => __bynkDecimalDiv(d("1"), d("0.00"), 2, "Down"), /division by zero/);
});

test("parse is strict; toString round-trips", () => {
  for (const ok of ["0", "-0", "1.20", "-0.05", "007", "123456789012345678901234567890.123"]) {
    assert.equal(__bynkDecimalParse(ok).tag, "Some", ok);
  }
  for (const bad of ["", "+1", "1e3", "1E3", ".5", "5.", " 1", "1 ", "1,5", "--1", "1.2.3", "0x10", "NaN"]) {
    assert.equal(__bynkDecimalParse(bad).tag, "None", JSON.stringify(bad));
  }
  assert.equal(__bynkDecimalParse("1".repeat(1001)).tag, "None");
  assert.equal(str(d("123456789012345678901234567890.123")), "123456789012345678901234567890.123");
  assert.equal(str(__bynkDecimal(-5n, 3)), "-0.005");
});

test("toInt is exact; fromInt is scale 0", () => {
  assert.deepEqual(__bynkDecimalToInt(d("42.00")), { tag: "Some", value: 42 });
  assert.equal(__bynkDecimalToInt(d("42.5")).tag, "None");
  assert.equal(__bynkDecimalToInt(d("9007199254740993")).tag, "None");
  assert.equal(str(__bynkDecimalFromInt(-7)), "-7");
});

test("__bynkDecimalShow prints a decimal wherever it sits, and never throws", () => {
  assert.equal(__bynkDecimalShow(d("1.10")), "1.10d");
  assert.equal(__bynkDecimalShow({ a: [d("2"), d("-0.5")], b: "x" }), '{"a":["2d","-0.5d"],"b":"x"}');
  assert.equal(__bynkDecimalShow(undefined), "undefined");
  assert.equal(__bynkDecimalShow(7n), "7");
});

test("round and div fault above the scale cap, not at it", () => {
  assert.equal(__BYNK_DECIMAL_MAX_SCALE, 100);
  assert.equal(__bynkDecimalRound(d("1"), 100, "Down").scale, 100);
  assert.throws(() => __bynkDecimalRound(d("1"), 101, "Down"), /0 to 100/);
  assert.equal(__bynkDecimalDiv(d("1"), d("3"), 100, "Down").scale, 100);
  assert.throws(() => __bynkDecimalDiv(d("1"), d("3"), 101, "Down"), /0 to 100/);
  assert.throws(() => __bynkDecimalRound(d("1"), -1, "Down"));
  assert.throws(() => __bynkDecimalRound(d("1"), 1.5, "Down"));
});

test("the wire decoder refuses leading zeros; parse stays lenient", () => {
  for (const ok of ["0", "-0", "0.5", "-0.50", "10", "10.05", "123.456"]) {
    assert.equal(__bynkDecimalParseWire(ok).tag, "Some", ok);
  }
  for (const bad of ["007", "-01.5", "00", "00.5", "+1", "1e3", ".5", "5.", ""]) {
    assert.equal(__bynkDecimalParseWire(bad).tag, "None", bad);
  }
  assert.equal(__bynkDecimalParse("007").tag, "Some");
});

test("every value an operation yields encodes to text that decodes back equal", () => {
  // The largest scale, and a magnitude whose text is exactly MAX_TEXT long.
  const widest = __bynkDecimalRound(d("1"), 100, "Down");
  assert.equal(__bynkDecimalToString(widest).length, 102);
  const back = __bynkDecimalParseWire(__bynkDecimalToString(widest));
  assert.equal(back.tag, "Some");
  assert.equal(__bynkDecimalEqual((back as { value: __BynkDecimalValue }).value, widest), true);

  const longest = "9".repeat(999);
  const big = d(longest);
  assert.equal(__bynkDecimalToString(big).length, 999);
  const exact = "-" + "9".repeat(999);
  assert.equal(__bynkDecimalToString(d(exact)).length, 1000);
  const rt = __bynkDecimalParseWire(__bynkDecimalToString(d(exact)));
  assert.equal(rt.tag, "Some");
  assert.equal(__bynkDecimalEqual((rt as { value: __BynkDecimalValue }).value, d(exact)), true);
});

test("an operation whose result breaks the invariant faults at the operation", () => {
  const x = d("0." + "1".repeat(60));
  assert.throws(() => __bynkDecimalMul(x, x), /scale 120 exceeds 100/);
  assert.throws(() => __bynkDecimalMul(d("9".repeat(600)), d("9".repeat(600))), /exceeds 1000 characters/);
  assert.throws(() => __bynkDecimalAdd(d("9".repeat(1000)), d("1")), /exceeds 1000 characters/);
  assert.equal(__bynkDecimalParse("0." + "1".repeat(101)).tag, "None");
  assert.equal(__bynkDecimalParse("0." + "1".repeat(100)).tag, "Some");
});
