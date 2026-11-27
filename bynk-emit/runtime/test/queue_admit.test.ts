import assert from "node:assert/strict";
import { test } from "node:test";
import {
  admitQueueBody,
  queueForeignKeys,
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
    // The fake's value is how long the claim holds: the lease when taken, the window once confirmed.
    claim: async (key, ttl) => {
      if (claimed.has(key)) return { claimed: false, remainingSeconds: claimed.get(key)! };
      claimed.set(key, ttl);
      calls.push("claim");
      return { claimed: true };
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

test("a redelivery that arrives while the lease is held is retried after it, never acknowledged as a replay", async () => {
  // The instance died (or its settle failed) mid-handler: nothing settled, and the platform redelivers at once.
  const p = ports({ "shop.orders": ["k1"] });
  const envelope = wire(await signQueueMessage("k1", parts));
  admitted(await queueAdmit(envelope, "fulfil", "orders", p));
  const early = await queueAdmit(envelope, "fulfil", "orders", p);
  assert.deepEqual(early, { ok: false, reason: "in_flight", retryAfterSeconds: QUEUE_CLAIM_LEASE_SECONDS });
  // A lease with 90.2 seconds left is retried after 91 seconds, and at least one second.
  p.claimed.set([...p.claimed.keys()][0]!, 90.2);
  assert.deepEqual(await queueAdmit(envelope, "fulfil", "orders", p), { ok: false, reason: "in_flight", retryAfterSeconds: 91 });
  p.claimed.set([...p.claimed.keys()][0]!, 0);
  assert.deepEqual(await queueAdmit(envelope, "fulfil", "orders", p), { ok: false, reason: "in_flight", retryAfterSeconds: 1 });
  assert.equal(p.claimed.size, 1);
});

test("a message whose handler failed is admitted again when the platform redelivers it", async () => {
  const p = ports({ "shop.orders": ["k1"] });
  const envelope = wire(await signQueueMessage("k1", parts));
  // While the handler runs, a second delivery of the same id waits: it is retried after the lease, not dropped.
  const first = admitted(await queueAdmit(envelope, "fulfil", "orders", p));
  assert.deepEqual(await queueAdmit(envelope, "fulfil", "orders", p), { ok: false, reason: "in_flight", retryAfterSeconds: QUEUE_CLAIM_LEASE_SECONDS });
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
  const reasons = ["unsigned", "malformed", "version", "unknown_sender", "bad_signature", "replayed", "stale", "in_flight"] as const;
  const texts = new Set(reasons.map((r) => queueRefusalDetail("fulfil", r)));
  assert.equal(texts.size, reasons.length);
  for (const t of texts) assert.ok(t.includes("fulfil"));
});

// ---- A consumer's own keys and id claim (#3899 3c-1) ----

const secrets = { Legacy: "LEGACY_QUEUE_KEY" };
// The part of D1 the claims use: `held` maps a key to its expiry (ms), and each statement is read by its SQL.
const claimDb = () => {
  const held = new Map<string, number>();
  return {
    held,
    prepare: (sql: string) => ({
      bind: (...values: unknown[]) => ({
        sql,
        values,
        // The read of a holder's expiry after a failed claim.
        first: async () => (held.has(String(values[0])) ? { expires_at: held.get(String(values[0])) } : null),
      }),
    }),
    batch: async (statements: unknown[]) => {
      const first = statements[0] as { sql: string; values: unknown[] };
      const key = String(first.values[0]);
      if (first.sql.startsWith("DELETE")) {
        held.delete(key);
        return [{ meta: { changes: 1 } }];
      }
      const now = Date.now();
      if (first.sql.includes("WHERE idempotency_claims.expires_at")) {
        // The claim: only when the key is absent or lapsed.
        const live = (held.get(key) ?? 0) > now;
        if (!live) held.set(key, Number(first.values[1]));
        return [{ meta: { changes: live ? 0 : 1 } }, { meta: { changes: 0 } }];
      }
      held.set(key, Number(first.values[1])); // the confirm
      return [{ meta: { changes: 1 } }];
    },
  };
};
const nowSecs = () => Math.floor(Date.now() / 1000);
const legacy = (over: Partial<typeof parts> = {}) => signQueueMessage("legacy-secret", { ...parts, from: "Legacy", ts: nowSecs(), ...over });

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

test("a signed message is admitted through env's secret, leased, and kept for the window once handled", async () => {
  const db = claimDb();
  const env = { LEGACY_QUEUE_KEY: "legacy-secret", D1_IDEMPOTENCY: db };
  const envelope = wire(await legacy());
  const first = admitted(await admitQueueBody(envelope, "orders", [], env, secrets));
  assert.deepEqual([first.from, first.id, first.body], ["Legacy", "m-1", { total: 5 }]);
  // While it is being handled, the redelivery waits for the lease rather than being dropped.
  const early = await admitQueueBody(envelope, "orders", [], env, secrets);
  assert.equal(early.ok === false && early.reason, "in_flight");
  await first.settle(true);
  const [until] = [...db.held.values()];
  assert.ok(until! > Date.now() + (QUEUE_DEDUP_WINDOW_SECONDS - 60) * 1000, "kept for the replay window");
  assert.deepEqual(await admitQueueBody(envelope, "orders", [], env, secrets), { ok: false, reason: "replayed" });
});

test("a message whose handler failed is admitted again when it is redelivered", async () => {
  const db = claimDb();
  const env = { LEGACY_QUEUE_KEY: "legacy-secret", D1_IDEMPOTENCY: db };
  const envelope = wire(await legacy());
  await admitted(await admitQueueBody(envelope, "orders", [], env, secrets)).settle(false);
  assert.equal(db.held.size, 0);
  assert.equal((await admitQueueBody(envelope, "orders", [], env, secrets)).ok, true);
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

// A Bynk producer signs under the key of its edge to the consumer's context
// (`BYNK_CALL_FROM_<PRODUCER>` here), and a `Producer` refinement decides which contexts are admitted.
const fromOrders = (over: Partial<typeof parts> = {}) => signQueueMessage("edge-secret", { ...parts, ts: nowSecs(), ...over });
const orders = (c: string) => c === "shop.orders";

test("a context a Producer refinement admits is verified under its edge key, and reported as a context", async () => {
  const env = { BYNK_CALL_FROM_SHOP__ORDERS: "edge-secret", D1_IDEMPOTENCY: claimDb() };
  const r = admitted(await admitQueueBody(wire(await fromOrders()), "orders", [], env, {}, orders));
  assert.deepEqual([r.from, r.context], ["shop.orders", true]);
  // A declared foreign sender is not a context.
  const both = { ...env, LEGACY_QUEUE_KEY: "legacy-secret" };
  assert.equal(admitted(await admitQueueBody(wire(await legacy({ id: "m-9" })), "orders", [], both, secrets, orders)).context, false);
});

test("a context the refinement does not admit, or whose edge key is unset, is refused as an unknown sender", async () => {
  const env = { BYNK_CALL_FROM_SHOP__ORDERS: "edge-secret", BYNK_CALL_FROM_SHOP__OTHER: "other-secret", D1_IDEMPOTENCY: claimDb() };
  const other = wire(await signQueueMessage("other-secret", { ...parts, from: "shop.other", ts: nowSecs() }));
  assert.deepEqual(await admitQueueBody(other, "orders", [], env, {}, orders), { ok: false, reason: "unknown_sender" });
  const unset = { D1_IDEMPOTENCY: claimDb() };
  assert.deepEqual(await admitQueueBody(wire(await fromOrders()), "orders", [], unset, {}, orders), { ok: false, reason: "unknown_sender" });
  // No refinement admits no context: a signed message from one is refused, whatever key is set.
  assert.deepEqual(await admitQueueBody(wire(await fromOrders()), "orders", [], env, secrets), { ok: false, reason: "unknown_sender" });
});

test("a message signed under another context's key, or a foreign secret, is not admitted as the context", async () => {
  const env = { BYNK_CALL_FROM_SHOP__ORDERS: "edge-secret", D1_IDEMPOTENCY: claimDb() };
  const forged = wire(await signQueueMessage("wrong-secret", { ...parts, ts: nowSecs() }));
  assert.deepEqual(await admitQueueBody(forged, "orders", [], env, {}, orders), { ok: false, reason: "bad_signature" });
  // A predicate that admits every name does not let a foreign actor's name through as a context.
  const everyone = () => true;
  const asLegacy = wire(await legacy({ id: "m-8" }));
  assert.deepEqual(await admitQueueBody(asLegacy, "orders", [], { D1_IDEMPOTENCY: claimDb() }, {}, everyone), { ok: false, reason: "unknown_sender" });
});

// #4526: a rotation leaves the edge with two keys, the new one in the slot the live one was not in and
// the old one marked with the instant it stops being verified under. A message signed under the old key
// before the rotation is admitted until then, and refused (not lost: the consumer reports the refusal)
// from the instant it passes, whether or not another deploy runs.
const secretsOf = (over: Record<string, string>) => ({ D1_IDEMPOTENCY: claimDb(), ...over });
const UNTIL = Math.floor(NOW / 1000) + 3600;

test("a rotation: a message signed under the previous edge key is admitted until its window ends", async () => {
  const env = secretsOf({
    BYNK_CALL_FROM_SHOP__ORDERS: "old-secret",
    BYNK_CALL_UNTIL_SHOP__ORDERS: String(UNTIL),
    BYNK_CALL_FROM2_SHOP__ORDERS: "new-secret",
  });
  const oldSigned = await signQueueMessage("old-secret", { ...parts, id: "old-1" });
  const newSigned = await signQueueMessage("new-secret", { ...parts, id: "new-1" });
  const during = queueForeignKeys(env, {}, orders, () => (UNTIL - 1) * 1000)("shop.orders");
  assert.deepEqual(during, ["new-secret", "old-secret"], "the live key first");
  const clock = () => NOW;
  assert.equal(admitted(await admitQueueBody(wire(oldSigned), "orders", [], env, {}, orders, clock)).from, "shop.orders");
  assert.equal(admitted(await admitQueueBody(wire(newSigned), "orders", [], env, {}, orders, clock)).from, "shop.orders");
});

test("a rotation: the previous edge key is refused from the instant its window ends, with no deploy in between", async () => {
  const env = {
    BYNK_CALL_FROM_SHOP__ORDERS: "old-secret",
    BYNK_CALL_UNTIL_SHOP__ORDERS: String(UNTIL),
    BYNK_CALL_FROM2_SHOP__ORDERS: "new-secret",
  };
  const at = (seconds: number) => queueForeignKeys(env, {}, orders, () => seconds * 1000)("shop.orders");
  assert.deepEqual(at(UNTIL - 1), ["new-secret", "old-secret"]);
  assert.deepEqual(at(UNTIL), ["new-secret"], "the instant itself is past the window");
  assert.deepEqual(at(UNTIL + 86_400 * 365), ["new-secret"]);
  // And a message signed under the old key, arriving after, is refused as a bad signature.
  const late = { ...env, D1_IDEMPOTENCY: claimDb() };
  const oldSigned = wire(await signQueueMessage("old-secret", { ...parts, id: "old-2" }));
  assert.deepEqual(await admitQueueBody(oldSigned, "orders", [], late, {}, orders, () => (UNTIL + 1) * 1000), { ok: false, reason: "bad_signature" });
});

test("a rotation works from either slot: the new key in the first, the old one in the second", () => {
  const env = {
    BYNK_CALL_FROM_SHOP__ORDERS: "new-secret",
    BYNK_CALL_FROM2_SHOP__ORDERS: "old-secret",
    BYNK_CALL_UNTIL2_SHOP__ORDERS: String(UNTIL),
  };
  assert.deepEqual(queueForeignKeys(env, {}, orders, () => 0)("shop.orders"), ["new-secret", "old-secret"]);
  assert.deepEqual(queueForeignKeys(env, {}, orders, () => UNTIL * 1000)("shop.orders"), ["new-secret"]);
});

test("a retirement marker that is missing a number, or is garbled, refuses its slot: the check fails closed", () => {
  for (const marker of ["", "soon", "-1", "12.5", "1e9", " 99999999999", "9".repeat(16), "0x10"]) {
    const env = { BYNK_CALL_FROM_SHOP__ORDERS: "old-secret", BYNK_CALL_UNTIL_SHOP__ORDERS: marker, BYNK_CALL_FROM2_SHOP__ORDERS: "new-secret" };
    assert.deepEqual(queueForeignKeys(env, {}, orders, () => 0)("shop.orders"), ["new-secret"], JSON.stringify(marker));
  }
  const notAString = { BYNK_CALL_FROM_SHOP__ORDERS: "old-secret", BYNK_CALL_UNTIL_SHOP__ORDERS: 99999999999, BYNK_CALL_FROM2_SHOP__ORDERS: "new-secret" };
  assert.deepEqual(queueForeignKeys(notAString, {}, orders, () => 0)("shop.orders"), ["new-secret"]);
});

test("a slot that holds no key, an unmarked second slot, and a name outside the edge domain", () => {
  const only2 = { BYNK_CALL_FROM2_SHOP__ORDERS: "k2" };
  assert.deepEqual(queueForeignKeys(only2, {}, orders, () => 0)("shop.orders"), ["k2"]);
  assert.deepEqual(queueForeignKeys({ BYNK_CALL_FROM_SHOP__ORDERS: "" }, {}, orders, () => 0)("shop.orders"), []);
  // Markers for a slot with no key admit nothing.
  assert.deepEqual(queueForeignKeys({ BYNK_CALL_UNTIL_SHOP__ORDERS: String(UNTIL) }, {}, orders, () => 0)("shop.orders"), []);
  assert.deepEqual(queueForeignKeys(only2, {}, () => true, () => 0)("Shop.Orders"), []);
});
