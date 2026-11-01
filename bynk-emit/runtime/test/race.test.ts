import { test } from "node:test";
import assert from "node:assert/strict";
import { __bynkRace } from "../src/settle.ts";

// Concurrent effects, slice 4 (#2711).
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

test("the first success wins", async () => {
  const e = exec();
  assert.equal(await __bynkRace([later(30, "slow"), later(2, "fast")], e), "fast");
});

test("every loser is handed to waitUntil, uncancelled", async () => {
  const e = exec();
  let finished = false;
  const slow = later(30, "slow").then((v) => {
    finished = true;
    return v;
  });
  const third = later(40, "third");
  assert.equal(await __bynkRace([slow, later(2, "fast"), third], e), "fast");
  // The race has returned and the losers have not finished: they were not cancelled.
  assert.equal(finished, false);
  assert.equal(e.kept.length, 2);
  await Promise.all(e.kept);
  assert.equal(finished, true);
});

test("a fast fault loses to a slower success", async () => {
  const e = exec();
  const r = await __bynkRace([laterFail(2, new Error("boom")), later(20, "ok")], e);
  assert.equal(r, "ok");
  // The fast fault is a loser like any other: handed to waitUntil.
  assert.equal(e.kept.length, 1);
});

test("when every side faults, the first fault in source order is raised with the rest as suppressed", async () => {
  const first = new Error("first");
  const second = new Error("second");
  const third = new Error("third");
  const e = exec();
  // The first in source order fails LAST in time.
  await assert.rejects(
    __bynkRace([laterFail(30, first), laterFail(2, second), laterFail(10, third)], e),
    (x) => {
      assert.equal(x, first);
      assert.deepEqual((x as { suppressed?: unknown }).suppressed, [second, third]);
      return true;
    },
  );
  assert.equal(e.kept.length, 0);
});

test("a single fault with no siblings left carries no suppressed", async () => {
  const boom = new Error("boom");
  await assert.rejects(__bynkRace([Promise.reject(boom), Promise.reject(boom)], exec()), (x) => x === boom);
});

test("a tie goes to the earlier side in source order", async () => {
  const e = exec();
  assert.equal(await __bynkRace([Promise.resolve("receiver"), Promise.resolve("argument")], e), "receiver");
  assert.equal(e.kept.length, 1);
});

test("a late fault in a loser is the platform's to log, not the call's", async () => {
  const e = exec();
  assert.equal(await __bynkRace([later(2, "ok"), laterFail(20, new Error("late"))], e), "ok");
  await Promise.all(e.kept);
});
