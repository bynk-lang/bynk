import { test } from "node:test";
import assert from "node:assert/strict";
import { __bynkLines, __bynkPad, __bynkRepeat } from "../src/text.ts";

// #3978: the String methods whose meaning the language fixes, not the host.

test("padStart and padEnd count code points, and an astral character is one position", () => {
  assert.equal(__bynkPad("42", 6, "0", true), "000042");
  assert.equal(__bynkPad("42", 6, "0", false), "420000");
  // "😀" is two UTF-16 units but one code point: 3 more positions to width 4.
  assert.equal(__bynkPad("😀", 4, "-", true), "---😀");
  // An astral `fill` is one position too, and is never cut in half.
  assert.equal(__bynkPad("a", 3, "😀", true), "😀😀a");
});

test("a multi-character fill is cut to fit, and an already-wide string is unchanged", () => {
  assert.equal(__bynkPad("x", 6, "ab", true), "ababax");
  assert.equal(__bynkPad("x", 6, "ab", false), "xababa");
  assert.equal(__bynkPad("hello", 3, "-", true), "hello");
  assert.equal(__bynkPad("hello", 5, "-", true), "hello");
  assert.equal(__bynkPad("hello", -2, "-", true), "hello");
});

test("an empty fill faults", () => {
  assert.throws(() => __bynkPad("a", 3, "", true), /fill/);
});

test("repeat makes n copies, and a negative count faults", () => {
  assert.equal(__bynkRepeat("ab", 3), "ababab");
  assert.equal(__bynkRepeat("ab", 0), "");
  assert.throws(() => __bynkRepeat("ab", -1), /negative/);
});

test("lines splits on LF and CRLF, not a lone CR, with no empty last line", () => {
  assert.deepEqual(__bynkLines("a\nb\nc"), ["a", "b", "c"]);
  assert.deepEqual(__bynkLines("a\r\nb\r\n"), ["a", "b"]);
  assert.deepEqual(__bynkLines("a\rb"), ["a\rb"]);
  assert.deepEqual(__bynkLines(""), []);
  assert.deepEqual(__bynkLines("\n"), [""]);
  assert.deepEqual(__bynkLines("a\n\nb"), ["a", "", "b"]);
});
