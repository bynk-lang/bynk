import { test } from "node:test";
import assert from "node:assert/strict";
import { Some, None } from "../src/result.ts";
import { __bynkPercentDecode, __bynkPercentEncode } from "../src/url.ts";

// Standard-library K2 (#2594).
test("percentEncode keeps the unreserved set and encodes every other UTF-8 byte", () => {
  assert.equal(__bynkPercentEncode("AZaz09-._~"), "AZaz09-._~");
  assert.equal(__bynkPercentEncode("a b&c=d/e?f#g"), "a%20b%26c%3Dd%2Fe%3Ff%23g");
  assert.equal(__bynkPercentEncode("!'()*+"), "%21%27%28%29%2A%2B");
  assert.equal(__bynkPercentEncode("é€😀"), "%C3%A9%E2%82%AC%F0%9F%98%80");
  assert.equal(__bynkPercentEncode("\ud800"), "%EF%BF%BD");
  assert.equal(__bynkPercentEncode(""), "");
});

test("percentDecode reads either case and keeps + as +", () => {
  assert.deepEqual(__bynkPercentDecode("a%20b%2fc%2Fd"), Some("a b/c/d"));
  assert.deepEqual(__bynkPercentDecode("%C3%A9%e2%82%ac"), Some("é€"));
  assert.deepEqual(__bynkPercentDecode("a+b"), Some("a+b"));
  assert.deepEqual(__bynkPercentDecode("plain é"), Some("plain é"));
  assert.deepEqual(__bynkPercentDecode(""), Some(""));
});

test("percentDecode is None for a malformed escape or bytes that are not UTF-8", () => {
  for (const s of ["%", "%2", "%G0", "a%zzb", "%C3", "%FF", "%C3%28"]) {
    assert.deepEqual(__bynkPercentDecode(s), None, s);
  }
});

test("percentDecode(percentEncode(s)) is Some(s)", () => {
  for (const s of ["", "hello", "a b&c=d", "é€😀", "%41", "100%", "+-*/", "\u0000\u007f"]) {
    assert.deepEqual(__bynkPercentDecode(__bynkPercentEncode(s)), Some(s), s);
  }
});
