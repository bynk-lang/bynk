import assert from "node:assert/strict";
import { test } from "node:test";
import { admitQueueBody, queueAdmit, queueForeignKeys, queueRefusalDetail, type QueueAdmitPorts } from "../src/queue_admit.ts";
import { QUEUE_DEDUP_WINDOW_SECONDS, signQueueMessage } from "../src/queue_signing.ts";

const parts = { from: "shop.orders", id: "m-1", ts: 1_700_000_000, queue: "orders", body: { total: 5 } };

function ports(keys: Record<string, string[]>): QueueAdmitPorts & { claimed: Map<string, number> } {
  const claimed = new Map<string, number>();
  return {
    claimed,
    keysFor: (from) => keys[from] ?? [],
    claim: async (key, ttl) => {
      if (claimed.has(key)) return false;
      claimed.set(key, ttl);
      return true;
    },
  };
}
const wire = (v: unknown) => JSON.parse(JSON.stringify(v));

test("a paired producer's message is admitted and unwrapped", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const r = await queueAdmit(wire(await signQueueMessage("k1", parts)), "fulfil", "orders", p);
  assert.deepEqual(r, { ok: true, from: "shop.orders", id: "m-1", body: { total: 5 } });
  assert.deepEqual([...p.claimed.values()], [QUEUE_DEDUP_WINDOW_SECONDS]);
});

test("an unsigned message is refused and claims nothing", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  assert.deepEqual(await queueAdmit({ total: 5 }, "fulfil", "orders", p), { ok: false, reason: "unsigned" });
  assert.equal(p.claimed.size, 0);
});

test("a message signed by a context the actor does not admit is refused", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const other = await signQueueMessage("k2", { ...parts, from: "shop.admin" });
  assert.deepEqual(await queueAdmit(wire(other), "fulfil", "orders", p), { ok: false, reason: "unknown_sender" });
  assert.equal(p.claimed.size, 0);
});

test("a message signed under the wrong key is refused", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const forged = await signQueueMessage("guess", parts);
  assert.deepEqual(await queueAdmit(wire(forged), "fulfil", "orders", p), { ok: false, reason: "bad_signature" });
  assert.equal(p.claimed.size, 0);
});

test("a message moved from another queue is refused", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const moved = await signQueueMessage("k1", { ...parts, queue: "audit" });
  assert.deepEqual(await queueAdmit(wire(moved), "fulfil", "orders", p), { ok: false, reason: "bad_signature" });
});

test("a replayed message is refused the second time", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const envelope = wire(await signQueueMessage("k1", parts));
  assert.equal((await queueAdmit(envelope, "fulfil", "orders", p)).ok, true);
  assert.deepEqual(await queueAdmit(envelope, "fulfil", "orders", p), { ok: false, reason: "replayed" });
});

test("a message signed under the previous key still verifies during a rotation", async () => {
  const p = ports({ "shop.orders": ["k2", "k1"] });
  assert.equal((await queueAdmit(wire(await signQueueMessage("k1", parts)), "fulfil", "orders", p)).ok, true);
});

test("a dead-letter consumer verifies a message under the source queue's name", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const envelope = wire(await signQueueMessage("k1", parts));
  assert.equal((await queueAdmit(envelope, "audit-dead", "orders", p)).ok, true);
  // Verified under the dead-letter queue's own name it would not: the source queue is what is signed.
  assert.equal((await queueAdmit(envelope, "audit-dead", "orders-dead", ports({ "shop.orders": ["k1"] }))).ok, false);
});

test("every refusal has a distinct fault text naming the consumer", () => {
  const reasons = ["unsigned", "malformed", "version", "unknown_sender", "bad_signature", "replayed"] as const;
  const texts = new Set(reasons.map((r) => queueRefusalDetail("fulfil", r)));
  assert.equal(texts.size, reasons.length);
  for (const t of texts) assert.ok(t.includes("fulfil"));
});

// ---- A consumer's own keys and id claim (#3899 3c-1) ----

const secrets = { Legacy: "LEGACY_QUEUE_KEY" };
const claimDb = () => {
  const held = new Set<string>();
  return {
    held,
    prepare: (sql: string) => ({
      bind: (...values: unknown[]) => ({ sql, values }),
    }),
    batch: async (statements: unknown[]) => {
      const insert = statements[0] as { values: unknown[] };
      const key = String(insert.values[0]);
      const changes = held.has(key) ? 0 : 1;
      held.add(key);
      return [{ meta: { changes } }, { meta: { changes: 0 } }];
    },
  };
};
const legacy = (over: Partial<typeof parts> = {}) => signQueueMessage("legacy-secret", { ...parts, from: "Legacy", ...over });

test("a foreign sender's key is read from env, the previous one after it", () => {
  const keys = queueForeignKeys({ LEGACY_QUEUE_KEY: "now", LEGACY_QUEUE_KEY_PREVIOUS: "before" }, secrets);
  assert.deepEqual(keys("Legacy"), ["now", "before"]);
  assert.deepEqual(keys("Other"), []);
  assert.deepEqual(keys("hasOwnProperty"), []);
});

test("a secret missing from env refuses the message: nothing is admitted without a key", async () => {
  const db = claimDb();
  for (const env of [{ D1_IDEMPOTENCY: db }, { D1_IDEMPOTENCY: db, LEGACY_QUEUE_KEY: "" }, { D1_IDEMPOTENCY: db, LEGACY_QUEUE_KEY: 5 }]) {
    const r = await admitQueueBody(wire(await legacy()), "orders", [], env, secrets);
    assert.deepEqual(r, { ok: false, reason: "unknown_sender" });
  }
  assert.equal(db.held.size, 0);
});

test("a signed message is admitted through env's secret and claimed once in the Idempotency database", async () => {
  const db = claimDb();
  const env = { LEGACY_QUEUE_KEY: "legacy-secret", D1_IDEMPOTENCY: db };
  const envelope = wire(await legacy());
  assert.deepEqual(await admitQueueBody(envelope, "orders", [], env, secrets), { ok: true, from: "Legacy", id: "m-1", body: { total: 5 } });
  assert.deepEqual(await admitQueueBody(envelope, "orders", [], env, secrets), { ok: false, reason: "replayed" });
  assert.equal(db.held.size, 1);
});

test("a Worker without the Idempotency database fails loudly rather than admitting unchecked", async () => {
  const envelope = wire(await legacy());
  await assert.rejects(admitQueueBody(envelope, "orders", [], { LEGACY_QUEUE_KEY: "legacy-secret" }, secrets), /D1_IDEMPOTENCY/);
});

test("a message a dead-letter consumer receives is verified under the queue that dead-lettered it", async () => {
  const db = claimDb();
  const env = { LEGACY_QUEUE_KEY: "legacy-secret", D1_IDEMPOTENCY: db };
  const moved = wire(await legacy());
  assert.equal((await admitQueueBody(moved, "orders-dead", ["orders"], env, secrets)).ok, true);
  const other = wire(await legacy({ queue: "elsewhere", id: "m-2" }));
  assert.deepEqual(await admitQueueBody(other, "orders-dead", ["orders"], env, secrets), { ok: false, reason: "bad_signature" });
});
