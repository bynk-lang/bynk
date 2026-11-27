// Durable event delivery, slice 3b (#3898): a subscriber's event queue entry. Each test names the
// design doc's decision (D2, D5, D9, D11) or §6 threat it covers.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  admitEventMessage,
  deliverEventMessage,
  eventRetryDelaySeconds,
  isD1DailyLimit,
  secondsUntilUtcMidnight,
  type EventQueueMessage,
  type EventQueueRoute,
} from "../src/queue_admit.ts";
import { signQueueMessage } from "../src/queue_signing.ts";
import { Err, Ok, type Result } from "../src/result.ts";
import { currentTraceId } from "../src/traceparent.ts";

const QUEUE = "bynk-ev-shop-ledger-onplaced-1a2b3c4d";
const route: EventQueueRoute = { subscriber: "shop.ledger.OnPlaced", event: "Placed", owner: "shop.order", deadLetter: "__BYNK_DLQ_X" };

// The part of D1 the claims use, as queue_admit.test.ts fakes it: `held` maps a key to its expiry (ms).
const claimDb = (fail?: () => Error) => {
  const held = new Map<string, number>();
  return {
    held,
    prepare: (sql: string) => ({
      bind: (...values: unknown[]) => ({
        sql,
        values,
        first: async () => (held.has(String(values[0])) ? { expires_at: held.get(String(values[0])) } : null),
      }),
    }),
    batch: async (statements: unknown[]) => {
      if (fail !== undefined) throw fail();
      const first = statements[0] as { sql: string; values: unknown[] };
      const key = String(first.values[0]);
      if (first.sql.startsWith("DELETE")) {
        held.delete(key);
        return [{ meta: { changes: 1 } }];
      }
      if (first.sql.includes("WHERE idempotency_claims.expires_at")) {
        const live = (held.get(key) ?? 0) > Date.now();
        if (!live) held.set(key, Number(first.values[1]));
        return [{ meta: { changes: live ? 0 : 1 } }, { meta: { changes: 0 } }];
      }
      held.set(key, Number(first.values[1]));
      return [{ meta: { changes: 1 } }];
    },
  };
};

type World = { env: Record<string, unknown>; db: ReturnType<typeof claimDb>; dead: unknown[]; reports: unknown[]; runs: unknown[] };
function world(db = claimDb()): World {
  const dead: unknown[] = [];
  return {
    db,
    dead,
    reports: [],
    runs: [],
    env: {
      BYNK_CALL_FROM_SHOP__ORDER: "edge-key",
      BYNK_CALL_FROM_SHOP__ADMIN: "admin-key",
      D1_IDEMPOTENCY: db,
      __BYNK_DLQ_X: { send: async (body: unknown) => void dead.push(body) },
    },
  };
}

type Verdict = { tag: "ack" } | { tag: "retry"; delaySeconds?: number } | undefined;
function message(body: unknown, attempts = 1): EventQueueMessage & { verdict: Verdict } {
  const m = {
    body: JSON.parse(JSON.stringify(body)),
    attempts,
    verdict: undefined as Verdict,
    ack() { m.verdict ??= { tag: "ack" }; },
    retry(options?: { delaySeconds: number }) { m.verdict ??= { tag: "retry", delaySeconds: options?.delaySeconds }; },
  };
  return m;
}

const eventBody = (over: Record<string, unknown> = {}, envelope: Record<string, unknown> = {}) => ({
  type: "Placed",
  payload: { id: "o1" },
  envelope: { eventId: "evt-1", publisherId: "shop.order", emittedAt: 0, schemaVersion: 1, ...envelope },
  ...over,
});

const signed = (body: unknown, key = "edge-key", from = "shop.order", id = "evt-1") =>
  signQueueMessage(key, { from, id, ts: Math.floor(Date.now() / 1000), queue: QUEUE, body });

// The subscriber's decoder: a payload whose `id` is a string. `skewed` refuses every payload, as a
// subscriber built against a different type would.
let skewed = false;
const decode = (payload: unknown): Result<unknown, unknown> => {
  const p = payload as { id?: unknown };
  if (skewed || typeof p.id !== "string") return Err({ kind: "StructuralMismatch", path: "$.payload.id" });
  return Ok(p);
};

async function deliver(w: World, m: EventQueueMessage, run?: (p: unknown, e: unknown) => Promise<unknown>, r: EventQueueRoute = route) {
  await deliverEventMessage(m, QUEUE, r, w.env, (e) => void w.reports.push(e), decode, run ?? (async (p) => void w.runs.push(p)));
}

/** Silence the entry's logs for one call, and return what it logged. */
async function quietly(f: () => Promise<void>): Promise<unknown[][]> {
  const logged: unknown[][] = [];
  const [error, log] = [console.error, console.log];
  console.error = (...a: unknown[]) => void logged.push(a);
  console.log = (...a: unknown[]) => void logged.push(a);
  try {
    await f();
  } finally {
    [console.error, console.log] = [error, log];
  }
  return logged;
}
const labels = (logged: unknown[][]) => logged.map((l) => l[0]);

test("D2: a message the owner signed is handled, its claim confirmed for the window, then acknowledged", async () => {
  const w = world();
  const m = message(await signed(eventBody()));
  await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "ack" });
  assert.deepEqual(w.runs, [{ id: "o1" }]);
  assert.equal(w.db.held.size, 1);
  assert.ok([...w.db.held.values()][0]! > Date.now() + 14 * 86_400_000, "kept for the dedup window");
});

test("D11: the same eventId delivered twice runs the handler once; the second is acknowledged as replayed", async () => {
  const w = world();
  const env = await signed(eventBody());
  const [first, second] = [message(env), message(env)];
  await quietly(() => deliver(w, first));
  const logged = await quietly(() => deliver(w, second));
  assert.deepEqual([first.verdict, second.verdict], [{ tag: "ack" }, { tag: "ack" }]);
  assert.equal(w.runs.length, 1);
  assert.deepEqual(labels(logged), ["EventDelivery replayed"]);
  assert.equal(w.reports.length, 0, "a replay is not a fault");
});

test("§6 injection: an unsigned message is acknowledged, reported, never handled, retried or dead-lettered", async () => {
  const w = world();
  const m = message(eventBody());
  const logged = await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "ack" });
  assert.deepEqual([w.runs.length, w.dead.length, w.db.held.size], [0, 0, 0]);
  assert.equal(w.reports.length, 1);
  assert.deepEqual(logged[0]![1], { event: "Placed", subscriber: "shop.ledger.OnPlaced", reason: "unsigned" });
});

test("§6 non-owner (ADR 0289): a message signed by another context is refused as an unknown sender, never handled", async () => {
  const w = world();
  const m = message(await signed(eventBody({}, { publisherId: "shop.admin" }), "admin-key", "shop.admin"));
  const logged = await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "ack" });
  assert.deepEqual([w.runs.length, w.dead.length, w.db.held.size], [0, 0, 0]);
  assert.equal((logged[0]![1] as { reason: string }).reason, "unknown_sender");
});

test("§6 forged from: a message claiming the owner but signed under another key is a bad signature", async () => {
  const w = world();
  const m = message(await signed(eventBody(), "admin-key"));
  const logged = await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "ack" });
  assert.equal(w.runs.length, 0);
  assert.equal((logged[0]![1] as { reason: string }).reason, "bad_signature");
});

test("§6 moved message: one signed for another subscriber's queue does not verify here", async () => {
  const w = world();
  const other = await signQueueMessage("edge-key", { from: "shop.order", id: "evt-1", ts: Math.floor(Date.now() / 1000), queue: "bynk-ev-shop-audit-9f8e7d6c", body: eventBody() });
  const m = message(other);
  const logged = await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "ack" });
  assert.equal((logged[0]![1] as { reason: string }).reason, "bad_signature");
});

test("D11 stale: a message signed more than 15 days ago is refused, never handled", async () => {
  const w = world();
  const old = await signQueueMessage("edge-key", { from: "shop.order", id: "evt-1", ts: Math.floor(Date.now() / 1000) - 16 * 86_400, queue: QUEUE, body: eventBody() });
  const m = message(old);
  const logged = await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "ack" });
  assert.equal(w.runs.length, 0);
  assert.equal((logged[0]![1] as { reason: string }).reason, "stale");
});

test("D11 in flight: a redelivery while the lease is held is retried after the lease, not acknowledged", async () => {
  const w = world();
  const env = await signed(eventBody());
  const first = message(env);
  let second: ReturnType<typeof message> | undefined;
  // The second delivery arrives while the first handler still runs.
  await quietly(() =>
    deliver(w, first, async () => {
      second = message(env);
      await deliver(w, second);
    }),
  );
  assert.equal(second!.verdict?.tag, "retry");
  assert.ok((second!.verdict as { delaySeconds: number }).delaySeconds > 800, "after the 15-minute lease");
  assert.deepEqual(first.verdict, { tag: "ack" });
});

test("D5: a faulting handler releases its claim and retries after retryDelay × attempts; the retry runs it again", async () => {
  const w = world();
  const env = await signed(eventBody());
  let calls = 0;
  const flaky = async () => {
    calls += 1;
    if (calls === 1) throw new Error("downstream is down");
  };
  const first = message(env, 1);
  const logged = await quietly(() => deliver(w, first, flaky));
  assert.deepEqual(first.verdict, { tag: "retry", delaySeconds: 10 });
  assert.equal(w.db.held.size, 0, "released");
  assert.equal(w.reports.length, 1);
  assert.ok(!labels(logged).includes("EventDelivery dead-lettered"));
  const again = message(env, 2);
  await quietly(() => deliver(w, again, flaky));
  assert.deepEqual(again.verdict, { tag: "ack" });
  assert.equal(calls, 2);
});

test("D5: the delay grows with the attempt; the last attempt (retries + 1) logs EventDelivery dead-lettered", async () => {
  const w = world();
  const env = await signed(eventBody());
  const boom = async () => {
    throw new Error("still down");
  };
  const delays: unknown[] = [];
  let last: unknown[][] = [];
  for (const attempts of [1, 2, 3, 4]) {
    const m = message(env, attempts);
    last = await quietly(() => deliver(w, m, boom));
    delays.push((m.verdict as { delaySeconds: number }).delaySeconds);
    if (attempts < 4) assert.ok(!labels(last).includes("EventDelivery dead-lettered"), `attempt ${attempts}`);
  }
  assert.deepEqual(delays, [10, 20, 30, 40]);
  const dead = last.find((l) => l[0] === "EventDelivery dead-lettered");
  assert.deepEqual(dead?.[1], { event: "Placed", subscriber: "shop.ledger.OnPlaced", eventId: "evt-1", attempts: 4 });
});

test("D5: the retry delay is clamped to Cloudflare's 0..86400 seconds", () => {
  assert.equal(eventRetryDelaySeconds(10, 3), 30);
  assert.equal(eventRetryDelaySeconds(50_000, 3), 86_400);
  assert.equal(eventRetryDelaySeconds(10, 0), 10);
  assert.equal(eventRetryDelaySeconds(Number.POSITIVE_INFINITY, 1), 86_400);
});

test("D2 step 2: a verified message whose publisherId disagrees with its signer is dead-lettered, its claim released", async () => {
  const w = world();
  const body = eventBody({}, { publisherId: "shop.admin" });
  const m = message(await signed(body));
  const logged = await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "ack" });
  assert.equal(w.runs.length, 0);
  assert.equal(w.dead.length, 1);
  assert.deepEqual(w.dead[0], m.body, "sent unchanged");
  assert.equal(w.db.held.size, 0, "released, so a redrive after the fix is admitted");
  assert.ok(labels(logged).includes("EventDelivery mismatched"));
  assert.equal(w.reports.length, 1);
});

test("D2 step 2: a verified message of another event type is dead-lettered, never handled", async () => {
  const w = world();
  const m = message(await signed(eventBody({ type: "Shipped" })));
  await quietly(() => deliver(w, m));
  assert.deepEqual([m.verdict, w.runs.length, w.dead.length, w.db.held.size], [{ tag: "ack" }, 0, 1, 0]);
});

test("D5 redrive after skew (R2): an undecodable message is dead-lettered with its claim released; the same message, redriven once decodable, runs the handler once", async () => {
  const w = world();
  const env = await signed(eventBody());
  skewed = true;
  const first = message(env);
  try {
    const logged = await quietly(() => deliver(w, first));
    assert.ok(labels(logged).includes("EventDelivery undecodable"));
  } finally {
    skewed = false;
  }
  assert.deepEqual(first.verdict, { tag: "ack" }, "never retried: the bytes will not change");
  assert.deepEqual([w.runs.length, w.dead.length, w.db.held.size], [0, 1, 0]);
  // The redrive: the dead letter, put back on its source queue by hand, still verifies.
  const redriven = message(w.dead[0]);
  await quietly(() => deliver(w, redriven));
  assert.deepEqual(redriven.verdict, { tag: "ack" });
  assert.equal(w.runs.length, 1);
  const replay = message(w.dead[0]);
  await quietly(() => deliver(w, replay));
  assert.equal(w.runs.length, 1, "and only once");
});

test("D5: a dead-letter queue that cannot take the message retries it rather than lose it", async () => {
  const w = world();
  delete w.env.__BYNK_DLQ_X;
  const m = message(await signed(eventBody({ payload: { id: 5 } })));
  const logged = await quietly(() => deliver(w, m));
  assert.equal(m.verdict?.tag, "retry");
  assert.ok(labels(logged).includes("EventDelivery dead-letter failed"));
  assert.equal(w.db.held.size, 0);
});

test("D9: a claim refused by D1's daily limit is retried at 00:00 UTC, logged once per isolate, and never run unchecked", async () => {
  const limit = () => new Error("D1_ERROR: Your account has exceeded D1's free tier daily row write limit. Upgrade to a paid plan or wait until tomorrow (midnight UTC) to continue.");
  const w = world(claimDb(limit));
  const env = await signed(eventBody());
  const first = message(env);
  const logged = await quietly(() => deliver(w, first));
  const second = message(env);
  const again = await quietly(() => deliver(w, second));
  assert.equal(w.runs.length, 0);
  for (const m of [first, second]) {
    assert.equal(m.verdict?.tag, "retry");
    const delay = (m.verdict as { delaySeconds: number }).delaySeconds;
    assert.ok(delay >= 1 && delay <= 86_400);
  }
  assert.deepEqual(labels(logged), ["EventDelivery deferred: D1 daily limit"]);
  assert.deepEqual(labels(again), [], "once per isolate");
  assert.equal(w.reports.length, 0);
});

test("R4: any other claim failure is a fault: retried after the backoff, never run unchecked", async () => {
  const w = world(claimDb(() => new Error("D1 is unreachable")));
  const m = message(await signed(eventBody()));
  await quietly(() => deliver(w, m));
  assert.deepEqual(m.verdict, { tag: "retry", delaySeconds: 10 });
  assert.equal(w.runs.length, 0);
  assert.equal(w.reports.length, 1);
});

test("D9: the daily-limit error is recognised by D1's documented text, and the delay runs to midnight UTC", () => {
  assert.ok(isD1DailyLimit(new Error("Your account has exceeded D1's free tier daily row read limit.")));
  assert.ok(!isD1DailyLimit(new Error("D1_ERROR: no such table")));
  assert.equal(secondsUntilUtcMidnight(Date.UTC(2026, 10, 27, 23, 59, 0)), 60);
  assert.equal(secondsUntilUtcMidnight(Date.UTC(2026, 10, 27, 0, 0, 0)), 86_400);
});

test("#3895 (L10): the handler runs in the emission's trace, carried in the signed envelope", async () => {
  const w = world();
  const trace = "4bf92f3577b34da6a3ce929d0e0e4736";
  const m = message(await signed(eventBody({}, { trace })));
  let seen: string | undefined = "unset";
  await quietly(() => deliver(w, m, async () => void (seen = currentTraceId())));
  assert.equal(seen, trace);
});

test("D2: admitEventMessage admits only the event's owner, under the subscriber's own D11 key", async () => {
  const w = world();
  const ok = await admitEventMessage(await signed(eventBody()), "shop.ledger.OnPlaced", QUEUE, "shop.order", w.env);
  assert.equal(ok.ok, true);
  assert.ok([...w.db.held.keys()][0]!.includes("shop.ledger.OnPlaced"));
  const admin = await admitEventMessage(await signed(eventBody(), "admin-key", "shop.admin", "evt-2"), "shop.ledger.OnPlaced", QUEUE, "shop.order", w.env);
  assert.deepEqual(admin, { ok: false, reason: "unknown_sender" });
});
