import { test } from "node:test";
import assert from "node:assert/strict";
import { __bynkSettleAll } from "../src/settle.ts";

// Concurrent effects, slice 0 (#2640).
const later = <T>(ms: number, v: T) => new Promise<T>((res) => setTimeout(() => res(v), ms));
const laterFail = (ms: number, e: unknown) =>
  new Promise<never>((_, rej) => setTimeout(() => rej(e), ms));

test("yields every value in source order, whatever order they settle in", async () => {
  const [a, b, c] = await __bynkSettleAll([() => later(15, "a"), () => later(1, 2), () => 3]);
  assert.deepEqual([a, b, c], ["a", 2, 3]);
});

test("an empty join yields an empty tuple", async () => {
  assert.deepEqual(await __bynkSettleAll([]), []);
});

test("raises the first rejection in SOURCE order, not time order", async () => {
  const slow = new Error("first in source");
  const fast = new Error("first in time");
  await assert.rejects(
    __bynkSettleAll([() => laterFail(20, slow), () => laterFail(1, fast)]),
    (e) => e === slow,
  );
});

test("waits for a slow non-faulting sibling before raising", async () => {
  let finished = false;
  const sibling = later(30, 1).then((v) => {
    finished = true;
    return v;
  });
  await assert.rejects(__bynkSettleAll([() => laterFail(1, new Error("boom")), () => sibling]), /boom/);
  assert.equal(finished, true, "the sibling had settled by the time the join raised");
});

test("later rejections ride on the raised one as `suppressed`", async () => {
  const e1 = new Error("one");
  const e2 = new Error("two");
  const e3 = new Error("three");
  try {
    await __bynkSettleAll([() => laterFail(3, e1), () => laterFail(1, e2), () => later(2, 0), () => laterFail(2, e3)]);
    assert.fail("should have raised");
  } catch (e) {
    assert.equal(e, e1);
    assert.deepEqual((e as { suppressed?: unknown }).suppressed, [e2, e3]);
  }
});

test("a non-object rejection is raised as is", async () => {
  await assert.rejects(__bynkSettleAll([() => laterFail(1, "plain"), () => laterFail(2, "other")]), (e) => e === "plain");
});

test("a synchronous throw is a rejected entry; earlier effects still settle, later ones still issue", async () => {
  const order: string[] = [];
  let earlierDone = false;
  const boom = new Error("sync");
  const sibling = new Error("later sibling");
  await assert.rejects(
    __bynkSettleAll([
      () => {
        order.push("first");
        return later(20, 1).then((v) => { earlierDone = true; return v; });
      },
      () => { order.push("second"); throw boom; },
      () => { order.push("third"); return laterFail(1, sibling); },
    ]),
    (e) => e === boom,
  );
  assert.equal(earlierDone, true, "the earlier effect settled before the join raised");
  assert.deepEqual(order, ["first", "second", "third"], "source issue order, no early exit");
});
