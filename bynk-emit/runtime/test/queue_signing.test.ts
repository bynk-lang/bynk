import assert from "node:assert/strict";
import { test } from "node:test";
import {
  canonicalJson,
  queueReplayKey,
  signQueueMessage,
  verifyQueueMessage,
} from "../src/queue_signing.ts";

const parts = { from: "shop.orders", id: "m-1", ts: 1_700_000_000, queue: "orders", body: { b: 2, a: [1, { d: 4, c: 3 }] } };
const keys = (from: string) => (from === "shop.orders" ? ["current-secret", "previous-secret"] : []);

test("canonicalJson sorts keys at every depth and leaves arrays in order", () => {
  assert.equal(canonicalJson({ b: 1, a: { z: 1, y: [2, { n: 1, m: 2 }] } }), '{"a":{"y":[2,{"m":2,"n":1}],"z":1},"b":1}');
  assert.equal(canonicalJson({ a: undefined, b: null }), '{"b":null}');
});

test("a message signed by a known sender verifies and unwraps to its body", async () => {
  const envelope = await signQueueMessage("current-secret", parts);
  assert.equal(envelope.bynk.v, 1);
  const r = await verifyQueueMessage(JSON.parse(JSON.stringify(envelope)), "orders", keys);
  assert.deepEqual(r, { ok: true, from: "shop.orders", id: "m-1", ts: 1_700_000_000, body: parts.body });
});

test("the body's key order does not change the signature", async () => {
  const envelope = await signQueueMessage("current-secret", parts);
  const reordered = { ...envelope, body: { a: [1, { c: 3, d: 4 }], b: 2 } };
  assert.equal((await verifyQueueMessage(reordered, "orders", keys)).ok, true);
});

test("an unsigned body is refused as unsigned", async () => {
  assert.deepEqual(await verifyQueueMessage({ id: 1 }, "orders", keys), { ok: false, reason: "unsigned" });
  assert.deepEqual(await verifyQueueMessage("text", "orders", keys), { ok: false, reason: "unsigned" });
  assert.deepEqual(await verifyQueueMessage(null, "orders", keys), { ok: false, reason: "unsigned" });
});

test("a message signed under the wrong key, or with a changed body, id or timestamp, is refused", async () => {
  const good = await signQueueMessage("current-secret", parts);
  const wrongKey = await signQueueMessage("other-secret", parts);
  assert.deepEqual(await verifyQueueMessage(wrongKey, "orders", keys), { ok: false, reason: "bad_signature" });
  assert.deepEqual(await verifyQueueMessage({ ...good, body: { a: 1 } }, "orders", keys), { ok: false, reason: "bad_signature" });
  assert.deepEqual(await verifyQueueMessage({ bynk: { ...good.bynk, id: "m-2" }, body: good.body }, "orders", keys), { ok: false, reason: "bad_signature" });
  assert.deepEqual(await verifyQueueMessage({ bynk: { ...good.bynk, ts: 1 }, body: good.body }, "orders", keys), { ok: false, reason: "bad_signature" });
});

test("a message moved from another queue is refused, because the signature names its source queue", async () => {
  const envelope = await signQueueMessage("current-secret", { ...parts, queue: "audit" });
  assert.deepEqual(await verifyQueueMessage(envelope, "orders", keys), { ok: false, reason: "bad_signature" });
  assert.equal((await verifyQueueMessage(envelope, "audit", keys)).ok, true);
});

test("a sender the consumer does not admit is refused before any key is tried", async () => {
  const envelope = await signQueueMessage("current-secret", { ...parts, from: "shop.legacy" });
  assert.deepEqual(await verifyQueueMessage(envelope, "orders", keys), { ok: false, reason: "unknown_sender" });
});

test("the previous key still verifies while a rotation is in its window", async () => {
  const old = await signQueueMessage("previous-secret", parts);
  assert.equal((await verifyQueueMessage(old, "orders", keys)).ok, true);
  assert.deepEqual(await verifyQueueMessage(old, "orders", (f) => (f === "shop.orders" ? ["current-secret"] : [])), {
    ok: false,
    reason: "bad_signature",
  });
});

test("a malformed or future-version envelope is refused", async () => {
  const good = await signQueueMessage("current-secret", parts);
  assert.deepEqual(await verifyQueueMessage({ bynk: { ...good.bynk, v: 2 }, body: good.body }, "orders", keys), { ok: false, reason: "version" });
  assert.deepEqual(await verifyQueueMessage({ bynk: { ...good.bynk, sig: "zz" }, body: good.body }, "orders", keys), { ok: false, reason: "malformed" });
  assert.deepEqual(await verifyQueueMessage({ bynk: { from: "shop.orders" }, body: 1 }, "orders", keys), { ok: false, reason: "malformed" });
  assert.deepEqual(await verifyQueueMessage({ bynk: good.bynk }, "orders", keys), { ok: false, reason: "malformed" });
});

test("a replay key is per consumer, source queue and message id", () => {
  assert.equal(queueReplayKey("shop.fulfil", "orders", "m-1"), "bynk.queue.shop.fulfil.orders.m-1");
  assert.notEqual(queueReplayKey("shop.fulfil", "orders", "m-1"), queueReplayKey("shop.audit", "orders", "m-1"));
});
