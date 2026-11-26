import { test } from "node:test";
import assert from "node:assert/strict";
import { inspect } from "node:util";
import { BynkSecret, __bynkSecret } from "../src/secret.ts";

const TEXT = "sk_live_hunter2-ünïcode";

test("reveal returns the text, the one deliberate exit", () => {
  assert.equal(__bynkSecret(TEXT).reveal(), TEXT);
});

test("every string form of a secret is <secret>, never its text", () => {
  const s = new BynkSecret(TEXT);
  const shown = [
    String(s),
    `${s}`,
    s + "",
    s.toString(),
    JSON.stringify(s),
    JSON.stringify({ k: s }),
    JSON.stringify([s]),
    inspect(s),
    inspect({ nested: { s } }, { depth: 5 }),
    new Error(`failed with ${s}`).message,
  ].join("\n");
  assert.ok(!shown.includes(TEXT), shown);
  assert.ok(!shown.includes("hunter2"), shown);
  assert.ok(shown.includes("<secret>"));
});

test("the text is not an own property: keys, a spread and a clone do not carry it", () => {
  const s = new BynkSecret(TEXT);
  assert.deepEqual(Object.keys(s), []);
  assert.deepEqual(Object.getOwnPropertyNames(s), []);
  assert.ok(!JSON.stringify({ ...s }).includes(TEXT));
  assert.ok(!JSON.stringify(structuredClone({ s: { ...s } })).includes(TEXT));
});

test("matches is true only for the exact text", () => {
  const s = new BynkSecret(TEXT);
  assert.equal(s.matches(TEXT), true);
  assert.equal(s.matches(TEXT + "x"), false);
  assert.equal(s.matches(TEXT.slice(0, -1)), false);
  assert.equal(s.matches(""), false);
  assert.equal(s.matches("x".repeat(TEXT.length)), false);
});

test("matches compares UTF-8 octets, so a different string of the same UTF-16 length differs", () => {
  assert.equal(new BynkSecret("é").matches("é"), true);
  assert.equal(new BynkSecret("é").matches("é"), false);
});

test("an empty secret matches only the empty string, with no early return on length", () => {
  const s = new BynkSecret("");
  assert.equal(s.matches(""), true);
  assert.equal(s.matches("a"), false);
  assert.equal(new BynkSecret("a").matches(""), false);
});

test("a prefix of the secret, or the secret as a prefix, does not match", () => {
  const s = new BynkSecret("abcdef");
  assert.equal(s.matches("abc"), false);
  assert.equal(s.matches("abcdefgh"), false);
  assert.equal(s.matches("abcdeg"), false);
});
