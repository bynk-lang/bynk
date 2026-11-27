import assert from "node:assert/strict";
import { test } from "node:test";
import {
  canonicalJson,
  EVENT_MESSAGE_BUDGET_BYTES,
  EVENT_NEAR_LIMIT_BYTES,
  eventMessageBytes,
  measureEvent,
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
  assert.deepEqual(await verifyQueueMessage({ bynk: { v: 1, from: "shop.orders" }, body: 1 }, "orders", keys), { ok: false, reason: "malformed" });
  assert.deepEqual(await verifyQueueMessage({ bynk: { from: "shop.orders" }, body: 1 }, "orders", keys), { ok: false, reason: "version" });
  assert.deepEqual(await verifyQueueMessage({ bynk: good.bynk }, "orders", keys), { ok: false, reason: "malformed" });
});

test("a fractional timestamp, or a line feed in from or id, is refused as malformed", async () => {
  const good = await signQueueMessage("current-secret", parts);
  assert.deepEqual(await verifyQueueMessage({ bynk: { ...good.bynk, ts: 1.5 }, body: good.body }, "orders", keys), { ok: false, reason: "malformed" });
  assert.deepEqual(await verifyQueueMessage({ bynk: { ...good.bynk, from: "shop.orders\n" }, body: good.body }, "orders", keys), { ok: false, reason: "malformed" });
  assert.deepEqual(await verifyQueueMessage({ bynk: { ...good.bynk, id: "m\n1" }, body: good.body }, "orders", keys), { ok: false, reason: "malformed" });
});

test("a future version is reported as a version even when its other fields are reshaped", async () => {
  assert.deepEqual(await verifyQueueMessage({ bynk: { v: 2, sig64: "abc" }, body: 1 }, "orders", keys), { ok: false, reason: "version" });
});

test("a body with a toJSON is signed as the transport would carry it", async () => {
  const when = new Date(Date.UTC(2026, 10, 25));
  const envelope = await signQueueMessage("current-secret", { ...parts, body: { at: when } });
  const carried = JSON.parse(JSON.stringify(envelope));
  assert.equal((await verifyQueueMessage(carried, "orders", keys)).ok, true);
});

test("a replay key is per consumer, source queue and message id", () => {
  assert.equal(queueReplayKey("shop.fulfil", "orders", "m-1"), "bynk.queue.shop.fulfil.orders.m-1");
  assert.notEqual(queueReplayKey("shop.fulfil", "orders", "m-1"), queueReplayKey("shop.audit", "orders", "m-1"));
});

// Durable event delivery §8.1 (#3898), D3: the size measurement `Events.emit` runs, warning only.
test("an event's message size is measured as its signed queue message (D3)", () => {
  const event = {
    type: "Paid",
    payload: { id: "a1" },
    envelope: { eventId: "e1", publisherId: "shop.billing", emittedAt: 1_700_000_000_123, schemaVersion: 1 },
  };
  const expected = JSON.stringify({
    bynk: { v: 1, from: "shop.billing", id: "e1", ts: 1_700_000_000, sig: "0".repeat(64) },
    body: event,
  });
  assert.equal(eventMessageBytes(event), new TextEncoder().encode(expected).length);
  // UTF-8, not UTF-16: a non-ASCII character counts its encoded bytes.
  const wide = { ...event, payload: { id: "é1" } };
  assert.equal(eventMessageBytes(wide), eventMessageBytes(event) + 1);
});

test("EventNearLimit warns once per event type, from 96,000 bytes, and returns the event (§8.1)", () => {
  const warnings: unknown[][] = [];
  const original = console.warn;
  console.warn = (...args: unknown[]) => void warnings.push(args);
  try {
    const envelope = { eventId: "e1", publisherId: "shop", emittedAt: 0, schemaVersion: 1 };
    const small = { type: "Small", payload: { s: "x" }, envelope };
    const near = { type: "Near", payload: { s: "x".repeat(EVENT_NEAR_LIMIT_BYTES) }, envelope };
    assert.equal(measureEvent(small), small);
    assert.equal(warnings.length, 0);
    assert.equal(measureEvent(near), near);
    assert.equal(measureEvent(near), near);
    assert.equal(warnings.length, 1);
    assert.equal(warnings[0][0], "EventNearLimit");
    assert.deepEqual(Object.keys(warnings[0][1] as object), ["event", "size", "budget"]);
    assert.equal((warnings[0][1] as { budget: number }).budget, EVENT_MESSAGE_BUDGET_BYTES);
  } finally {
    console.warn = original;
  }
});

// Review of #4628: the measurement sizes the wire form, not the in-memory value.
test("an event is measured as the wire carries it: Bytes as base64, Map and Set as arrays (D3)", () => {
  const envelope = { eventId: "e1", publisherId: "shop", emittedAt: 0, schemaVersion: 1 };
  const size = (payload: unknown) => eventMessageBytes({ type: "T", payload, envelope });
  const base = size({ v: "" });
  // 3,000 octets are 4,000 base64 characters on the wire, not ~10 bytes per octet.
  assert.equal(size({ v: new Uint8Array(3000) }) - base, 4000);
  // A Map is its entries and a Set its members, not `{}`.
  const big = new Map(Array.from({ length: 1000 }, (_, i) => [`k${i}`, "x".repeat(100)] as const));
  assert.ok(size({ v: big }) > 100_000);
  assert.ok(size({ v: new Set(["a", "b"]) }) > base);
});

test("a value JSON.stringify refuses never faults the emission (D3, warn only)", () => {
  const envelope = { eventId: "e1", publisherId: "shop", emittedAt: 0, schemaVersion: 1 };
  // A Decimal's coefficient is a bigint, which `JSON.stringify` alone refuses.
  const decimal = { type: "Priced", payload: { price: { coefficient: 120n, scale: 2 } }, envelope };
  assert.equal(measureEvent(decimal), decimal);
  const cyclic: Record<string, unknown> = {};
  cyclic.self = cyclic;
  const odd = { type: "Odd", payload: cyclic, envelope };
  assert.equal(measureEvent(odd), odd);
});

test("a type that stays small is sampled after 100 emissions, and a near one still warns", () => {
  const warnings: unknown[][] = [];
  const original = console.warn;
  console.warn = (...args: unknown[]) => void warnings.push(args);
  try {
    const envelope = { eventId: "e1", publisherId: "shop", emittedAt: 0, schemaVersion: 1 };
    let measured = 0;
    const counting = { type: "Sampled", envelope, get payload() { measured++; return { s: "x" }; } };
    for (let i = 0; i < 300; i++) measureEvent(counting);
    assert.ok(measured >= 100 && measured < 110, `measured ${measured} of 300`);
    // A near-limit emission on a sampled emission still warns.
    const near = { type: "Sampled", payload: { s: "x".repeat(EVENT_NEAR_LIMIT_BYTES) }, envelope };
    for (let i = 0; i < 200 && warnings.length === 0; i++) measureEvent(near);
    assert.equal(warnings.length, 1);
  } finally {
    console.warn = original;
  }
});
