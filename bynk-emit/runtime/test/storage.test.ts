import { test } from "node:test";
import assert from "node:assert/strict";
import { makeIntegrationRateLimit } from "../src/storage.ts";

// Standard-library P3 (#2342): the integration tier's rate limiter.
test("makeIntegrationRateLimit allows `limit` calls per key per fixed window", async () => {
  let now = 0;
  const rl = makeIntegrationRateLimit(2, 10, () => now);
  const allow = async (key: string) => (await rl.limit({ key })).success;
  assert.deepEqual([await allow("a"), await allow("a"), await allow("a")], [true, true, false]);
  assert.equal(await allow("b"), true, "each key counts on its own");
  now = 9_999;
  assert.equal(await allow("a"), false, "still the same window");
  now = 10_000;
  assert.equal(await allow("a"), true, "a new window starts over");
});

test("makeIntegrationRateLimit stands still without a clock", async () => {
  const rl = makeIntegrationRateLimit(1, 10);
  assert.deepEqual([(await rl.limit({ key: "a" })).success, (await rl.limit({ key: "a" })).success], [true, false]);
});
