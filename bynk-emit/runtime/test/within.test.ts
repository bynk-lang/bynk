import { test } from "node:test";
import assert from "node:assert/strict";
import { __bynkWithin } from "../src/settle.ts";

// Concurrent effects, slice 3 (#2683).
const later = <T>(ms: number, v: T) => new Promise<T>((res) => setTimeout(() => res(v), ms));
const laterFail = (ms: number, e: unknown) =>
  new Promise<never>((_, rej) => setTimeout(() => rej(e), ms));

// The execution context a Worker hands a handler: `waitUntil` keeps what it is
// given alive after the handler returns. Collected here so a test can wait for it.
const exec = () => {
  const kept: Promise<unknown>[] = [];
  return {
    kept,
    waitUntil(p: Promise<unknown>): void {
      kept.push(p.catch(() => undefined));
    },
  };
};

test("a fast effect yields Ok and is not handed to waitUntil", async () => {
  const e = exec();
  const r = await __bynkWithin(later(2, "v"), 200, e);
  assert.deepEqual(r, { tag: "Ok", value: "v" });
  assert.equal(e.kept.length, 0);
});

test("a fast effect clears its timer", async () => {
  const set: unknown[] = [];
  const cleared: unknown[] = [];
  const e = {
    ...exec(),
    timers: {
      set(_fn: () => void, _ms: number): unknown {
        const h = {};
        set.push(h);
        return h;
      },
      clear(h: unknown): void {
        cleared.push(h);
      },
    },
  };
  assert.deepEqual(await __bynkWithin(Promise.resolve(1), 5000, e), { tag: "Ok", value: 1 });
  assert.equal(set.length, 1);
  assert.deepEqual(cleared, set);
});

test("a fast effect leaves no host timer pending", async () => {
  const before = process.getActiveResourcesInfo().filter((n) => n === "Timeout").length;
  await __bynkWithin(Promise.resolve(1), 60_000, exec());
  const after = process.getActiveResourcesInfo().filter((n) => n === "Timeout").length;
  assert.equal(after, before);
});

test("a slow effect yields Err(TimedOut { after })", async () => {
  const e = exec();
  const r = await __bynkWithin(later(60, "v"), 5, e);
  assert.deepEqual(r, { tag: "Err", error: { after: 5 } });
});

test("the abandoned effect still completes, after the handler has returned", async () => {
  const e = exec();
  let finished = false;
  const effect = later(30, "v").then((v) => {
    finished = true;
    return v;
  });
  const handler = async () => (await __bynkWithin(effect, 5, e)).tag;
  assert.equal(await handler(), "Err");
  // The handler has returned and the effect has not finished: it was not cancelled.
  assert.equal(finished, false);
  assert.equal(e.kept.length, 1);
  await Promise.all(e.kept);
  assert.equal(finished, true);
});

test("a tie goes to the effect", async () => {
  const e = exec();
  const queue: Array<() => void> = [];
  const timers = {
    set(fn: () => void): unknown {
      queue.push(fn);
      return fn;
    },
    clear(): void {},
  };
  let settle!: (v: string) => void;
  const effect = new Promise<string>((res) => (settle = res));
  const result = __bynkWithin(effect, 10, { ...e, timers });
  // The effect settles in the same turn as the deadline; the effect is first.
  settle("v");
  await Promise.resolve();
  queue[0]!();
  assert.deepEqual(await result, { tag: "Ok", value: "v" });
});

test("an effect that faults before the deadline faults the call", async () => {
  const boom = new Error("boom");
  const e = exec();
  await assert.rejects(__bynkWithin(laterFail(2, boom), 200, e), (x) => x === boom);
  assert.equal(e.kept.length, 0);
});

test("a fault after the deadline is the platform's to log, not the call's", async () => {
  const e = exec();
  const r = await __bynkWithin(laterFail(30, new Error("late")), 5, e);
  assert.equal(r.tag, "Err");
  await Promise.all(e.kept);
});

test("a deadline outside (0, 2^31 - 1] faults, handing the effect on", async () => {
  for (const ms of [0, -1, 2147483648, Number.NaN]) {
    const e = exec();
    await assert.rejects(__bynkWithin(Promise.resolve(1), ms, e), /Duration over zero/);
    assert.equal(e.kept.length, 1);
  }
});
