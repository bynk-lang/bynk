import assert from "node:assert/strict";
import { test } from "node:test";
import {
  QUEUE_CLAIM_LEASE_SECONDS,
  QUEUE_CLOCK_SKEW_SECONDS,
  queueAdmit,
  queueRefusalDetail,
  type QueueAdmission,
  type QueueAdmitPorts,
} from "../src/queue_admit.ts";
import { QUEUE_DEDUP_WINDOW_SECONDS, signQueueMessage } from "../src/queue_signing.ts";

const parts = { from: "shop.orders", id: "m-1", ts: 1_700_000_000, queue: "orders", body: { total: 5 } };
const NOW = (parts.ts + 60) * 1000;

type Fake = QueueAdmitPorts & { claimed: Map<string, number>; calls: string[] };
function ports(keys: Record<string, string[]>, now = NOW): Fake {
  const claimed = new Map<string, number>();
  const calls: string[] = [];
  return {
    claimed,
    calls,
    now: () => now,
    keysFor: (from) => keys[from] ?? [],
    claim: async (key, ttl) => {
      if (claimed.has(key)) return false;
      claimed.set(key, ttl);
      calls.push("claim");
      return true;
    },
    confirm: async (key, ttl) => {
      claimed.set(key, ttl);
      calls.push("confirm");
    },
    release: async (key) => {
      claimed.delete(key);
      calls.push("release");
    },
  };
}
const wire = (v: unknown) => JSON.parse(JSON.stringify(v));
const admitted = (r: QueueAdmission) => {
  assert.equal(r.ok, true);
  return r as Extract<QueueAdmission, { ok: true }>;
};

test("a paired producer's message is admitted and unwrapped, its id held on a short lease", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const r = admitted(await queueAdmit(wire(await signQueueMessage("k1", parts)), "fulfil", "orders", p));
  assert.deepEqual([r.from, r.id, r.body], ["shop.orders", "m-1", { total: 5 }]);
  assert.deepEqual([...p.claimed.values()], [QUEUE_CLAIM_LEASE_SECONDS]);
});

test("an unsigned message is refused and claims nothing", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  assert.deepEqual(await queueAdmit({ total: 5 }, "fulfil", "orders", p), { ok: false, reason: "unsigned" });
  assert.equal(p.claimed.size, 0);
});

test("a message signed by a context the actor does not admit is refused and claims nothing", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const other = await signQueueMessage("k2", { ...parts, from: "shop.admin" });
  assert.deepEqual(await queueAdmit(wire(other), "fulfil", "orders", p), { ok: false, reason: "unknown_sender" });
  assert.equal(p.claimed.size, 0);
});

test("a message signed under the wrong key is refused and claims nothing", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const forged = await signQueueMessage("guess", parts);
  assert.deepEqual(await queueAdmit(wire(forged), "fulfil", "orders", p), { ok: false, reason: "bad_signature" });
  assert.equal(p.claimed.size, 0);
});

test("a message moved from another queue is refused and claims nothing", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const moved = await signQueueMessage("k1", { ...parts, queue: "audit" });
  assert.deepEqual(await queueAdmit(wire(moved), "fulfil", "orders", p), { ok: false, reason: "bad_signature" });
  assert.equal(p.claimed.size, 0);
});

test("a malformed envelope and an unsupported version are refused and claim nothing", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const signed = wire(await signQueueMessage("k1", parts));
  assert.deepEqual(await queueAdmit({ ...signed, bynk: { ...signed.bynk, ts: 1.5 } }, "fulfil", "orders", p), { ok: false, reason: "malformed" });
  assert.deepEqual(await queueAdmit({ ...signed, bynk: { ...signed.bynk, v: 2 } }, "fulfil", "orders", p), { ok: false, reason: "version" });
  assert.equal(p.claimed.size, 0);
});

test("a message signed too long ago, or too far ahead, is refused and claims nothing", async () => {
  const signed = wire(await signQueueMessage("k1", parts));
  const old = ports({ "shop.orders": ["k1"] }, (parts.ts + QUEUE_DEDUP_WINDOW_SECONDS + 1) * 1000);
  assert.deepEqual(await queueAdmit(signed, "fulfil", "orders", old), { ok: false, reason: "stale" });
  const early = ports({ "shop.orders": ["k1"] }, (parts.ts - QUEUE_CLOCK_SKEW_SECONDS - 1) * 1000);
  assert.deepEqual(await queueAdmit(signed, "fulfil", "orders", early), { ok: false, reason: "stale" });
  assert.equal(old.claimed.size + early.claimed.size, 0);
  // At the edges, and a little ahead for clock skew, it is still admitted.
  const edge = ports({ "shop.orders": ["k1"] }, (parts.ts + QUEUE_DEDUP_WINDOW_SECONDS) * 1000);
  assert.equal((await queueAdmit(signed, "fulfil", "orders", edge)).ok, true);
  const skewed = ports({ "shop.orders": ["k1"] }, (parts.ts - QUEUE_CLOCK_SKEW_SECONDS) * 1000);
  assert.equal((await queueAdmit(signed, "fulfil", "orders", skewed)).ok, true);
});

test("a message that was handled is kept for the window, and a replay is refused", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const envelope = wire(await signQueueMessage("k1", parts));
  await admitted(await queueAdmit(envelope, "fulfil", "orders", p)).settle(true);
  assert.deepEqual([...p.claimed.values()], [QUEUE_DEDUP_WINDOW_SECONDS]);
  assert.deepEqual(await queueAdmit(envelope, "fulfil", "orders", p), { ok: false, reason: "replayed" });
});

test("a message whose handler failed is admitted again when the platform redelivers it", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const envelope = wire(await signQueueMessage("k1", parts));
  // While the handler runs, a second delivery of the same id is refused.
  const first = admitted(await queueAdmit(envelope, "fulfil", "orders", p));
  assert.deepEqual(await queueAdmit(envelope, "fulfil", "orders", p), { ok: false, reason: "replayed" });
  // The handler asked for a retry, or threw: the claim is released, and the redelivery is admitted.
  await first.settle(false);
  assert.equal(p.claimed.size, 0);
  const again = admitted(await queueAdmit(envelope, "fulfil", "orders", p));
  await again.settle(true);
  assert.deepEqual(await queueAdmit(envelope, "fulfil", "orders", p), { ok: false, reason: "replayed" });
});

test("a message signed under the previous key still verifies during a rotation", async () => {
  const p = ports({ "shop.orders": ["k2", "k1"] });
  assert.equal((await queueAdmit(wire(await signQueueMessage("k1", parts)), "fulfil", "orders", p)).ok, true);
});

test("a dead-letter consumer verifies a message under the source queue's name", async () => {
  const envelope = wire(await signQueueMessage("k1", parts));
  assert.equal((await queueAdmit(envelope, "audit-dead", "orders", ports({ "shop.orders": ["k1"] }))).ok, true);
  // Verified under the dead-letter queue's own name it would not: the source queue is what is signed.
  const p = ports({ "shop.orders": ["k1"] });
  assert.equal((await queueAdmit(envelope, "audit-dead", "orders-dead", p)).ok, false);
  assert.equal(p.claimed.size, 0);
});

test("every refusal has a distinct fault text naming the consumer", () => {
  const reasons = ["unsigned", "malformed", "version", "unknown_sender", "bad_signature", "replayed", "stale"] as const;
  const texts = new Set(reasons.map((r) => queueRefusalDetail("fulfil", r)));
  assert.equal(texts.size, reasons.length);
  for (const t of texts) assert.ok(t.includes("fulfil"));
});
