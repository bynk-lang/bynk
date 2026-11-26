import { test } from "node:test";
import assert from "node:assert/strict";
import { __bynkSettleAll, __bynkSettlePool } from "../src/settle.ts";

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

// #3968: the bounded pool.
function gauge() {
  let inFlight = 0;
  let peak = 0;
  const step = <T>(ms: number, v: T) => () => {
    inFlight += 1;
    peak = Math.max(peak, inFlight);
    return later(ms, v).finally(() => {
      inFlight -= 1;
    });
  };
  return { step, peak: () => peak };
}

test("pool: never more than `limit` in flight, results in input order", async () => {
  const g = gauge();
  const out = await __bynkSettlePool([g.step(10, "a"), g.step(1, "b"), g.step(5, "c"), g.step(1, "d"), g.step(1, "e")], 2);
  assert.deepEqual(out, ["a", "b", "c", "d", "e"]);
  assert.equal(g.peak(), 2);
});

test("pool: the next step starts when any settles, not in batches", async () => {
  const started: string[] = [];
  const mk = (name: string, ms: number) => () => {
    started.push(name);
    return later(ms, name);
  };
  // `slow` holds one slot; `b` and `c` should cycle through the other without waiting for `slow`.
  await __bynkSettlePool([mk("slow", 40), mk("b", 1), mk("c", 1), mk("d", 1)], 2);
  assert.deepEqual(started, ["slow", "b", "c", "d"]);
});

test("pool: a limit above the length runs everything at once", async () => {
  const g = gauge();
  await __bynkSettlePool([g.step(5, 1), g.step(5, 2), g.step(5, 3)], 1000);
  assert.equal(g.peak(), 3);
});

test("pool: an empty list yields an empty array", async () => {
  assert.deepEqual(await __bynkSettlePool([], 3), []);
});

test("pool: waits for in-flight steps, then raises the first fault in INPUT order", async () => {
  const slow = new Error("first in input order");
  const fast = new Error("first in time");
  let finished = false;
  const sibling = () => later(30, 1).then((v) => ((finished = true), v));
  await assert.rejects(
    __bynkSettlePool([() => laterFail(20, slow), () => laterFail(1, fast), sibling], 3),
    (e) => e === slow && (e as { suppressed?: unknown[] }).suppressed?.[0] === fast,
  );
  assert.equal(finished, true);
});

test("pool: a synchronous throw is a fault of that step only", async () => {
  const boom = new Error("sync");
  let ran = 0;
  await assert.rejects(
    __bynkSettlePool(
      [
        () => {
          ran += 1;
          return 1;
        },
        () => {
          throw boom;
        },
        () => {
          ran += 1;
          return 3;
        },
      ],
      1,
    ),
    (e) => e === boom,
  );
  assert.equal(ran, 2);
});

test("pool: stopOnErr starts no further step once an Err is seen, and waits for those in flight", async () => {
  const started: number[] = [];
  const err = { tag: "Err", error: "no" };
  const mk = (n: number, ms: number, v: unknown) => () => {
    started.push(n);
    return later(ms, v);
  };
  const out = await __bynkSettlePool(
    [mk(0, 1, err), mk(1, 20, { tag: "Ok", value: 1 }), mk(2, 1, { tag: "Ok", value: 2 }), mk(3, 1, { tag: "Ok", value: 3 })],
    2,
    true,
  );
  // 0 and 1 started together; 0 settled to an Err, so 2 and 3 never started; 1 was awaited.
  assert.deepEqual(started, [0, 1]);
  assert.deepEqual(out[0], err);
  assert.deepEqual(out[1], { tag: "Ok", value: 1 });
  assert.equal(out[2], undefined);
  assert.equal(out[3], undefined);
});

test("pool: a limit that is not a whole number from 1 to 1000 faults the call", async () => {
  for (const bad of [0, -1, 1001, 2.5, Number.NaN]) {
    await assert.rejects(__bynkSettlePool([() => 1], bad), /whole number from 1 to 1000/);
  }
});
