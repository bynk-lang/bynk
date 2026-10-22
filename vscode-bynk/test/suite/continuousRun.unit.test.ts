// #2269 (Test Explorer depth, Slice E): the Watch toggle's rerun policy — the
// `vscode`-free core in `continuousRun.ts`. The echo of a save is dropped upstream,
// in the refresh hub, and is covered by the integration test in
// `test_continuous.test.ts`.

import * as assert from "assert";

import { ContinuousScheduler, FailureOnce, QUIET_MS, RunSlot } from "../../src/continuousRun";
import type { Timers } from "../../src/refreshScheduler";

/** A manual clock: `advance(ms)` runs every timer that falls due. */
class FakeTimers implements Timers {
  private now = 0;
  private next = 1;
  private pending = new Map<number, { at: number; fn: () => void }>();

  set(fn: () => void, ms: number): unknown {
    const id = this.next++;
    this.pending.set(id, { at: this.now + ms, fn });
    return id;
  }

  clear(handle: unknown): void {
    this.pending.delete(handle as number);
  }

  advance(ms: number): void {
    const until = this.now + ms;
    for (;;) {
      const due = [...this.pending.entries()]
        .filter(([, t]) => t.at <= until)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      this.pending.delete(due[0]);
      this.now = due[1].at;
      due[1].fn();
    }
    this.now = until;
  }
}

/** Let the scheduler's pending promise continuations run. */
const flush = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));

/** A scheduler whose runs the test controls: each run is recorded, stays in flight
 *  until the test finishes it, and records being told to stop. */
function setup() {
  const timers = new FakeTimers();
  const runs: { slot: RunSlot; finish: () => void; stopped: boolean; fail: (e: unknown) => void }[] = [];
  let inFlight = 0;
  let peak = 0;
  const scheduler = new ContinuousScheduler(
    (slot) =>
      new Promise<void>((resolve, reject) => {
        inFlight++;
        peak = Math.max(peak, inFlight);
        const run = {
          slot,
          stopped: false,
          finish: () => {
            inFlight--;
            resolve();
          },
          fail: (e: unknown) => {
            inFlight--;
            reject(e);
          },
        };
        slot.onCancel(() => (run.stopped = true));
        runs.push(run);
      }),
    timers,
  );
  return { scheduler, timers, runs, peak: () => peak };
}

describe("ContinuousScheduler (#2269 DECISIONS A and B)", () => {
  it("reruns once the quiet period has passed, not before", async () => {
    const { scheduler, timers, runs } = setup();
    scheduler.signal();
    timers.advance(QUIET_MS - 1);
    await flush();
    assert.strictEqual(runs.length, 0);
    assert.ok(scheduler.waiting);
    timers.advance(1);
    await flush();
    assert.strictEqual(runs.length, 1);
    assert.ok(!scheduler.waiting);
  });

  it("coalesces a burst of changes into one rerun that starts after the last", async () => {
    const { scheduler, timers, runs } = setup();
    scheduler.signal();
    timers.advance(400);
    scheduler.signal();
    timers.advance(400);
    scheduler.signal();
    timers.advance(QUIET_MS - 1);
    await flush();
    assert.strictEqual(runs.length, 0, "the quiet period restarts at each change");
    timers.advance(1);
    await flush();
    assert.strictEqual(runs.length, 1);
    timers.advance(10 * QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 1, "and nothing else follows");
  });

  it("cancels the run in flight at once when a change arrives, and starts a new one after the quiet period", async () => {
    const { scheduler, timers, runs } = setup();
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 1);
    assert.ok(!runs[0].stopped);

    scheduler.signal();
    assert.ok(runs[0].stopped, "told to stop the moment the change arrives, not after the wait");
    assert.ok(runs[0].slot.cancelled);

    runs[0].finish();
    await flush();
    assert.strictEqual(runs.length, 1, "the new run waits out its quiet period");
    timers.advance(QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 2);
    assert.ok(!runs[1].stopped);
  });

  it("never starts a run while the cancelled one is still ending", async () => {
    const { scheduler, timers, runs, peak } = setup();
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 1, "the cancelled run has not ended, so the next waits");
    assert.ok(scheduler.running);
    runs[0].finish();
    await flush();
    assert.strictEqual(runs.length, 2);
    runs[1].finish();
    await flush();
    assert.strictEqual(peak(), 1, "at most one run at a time, throughout");
  });

  it("starts one run, not two, when a change lands while the cancelled run is still ending", async () => {
    const { scheduler, timers, runs, peak } = setup();
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    scheduler.signal();
    timers.advance(QUIET_MS); // the first wait is over, and it is waiting on the run to end
    await flush();
    scheduler.signal(); // a newer change overtakes it
    timers.advance(QUIET_MS);
    await flush();
    runs[0].finish();
    await flush();
    assert.strictEqual(runs.length, 2, "one new run for the two changes");
    runs[1].finish();
    await flush();
    timers.advance(10 * QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 2);
    assert.strictEqual(peak(), 1);
  });

  it("does not cancel a run that finished before the next change", async () => {
    const { scheduler, timers, runs } = setup();
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    runs[0].finish();
    await flush();
    assert.ok(!scheduler.running);
    scheduler.signal();
    assert.ok(!runs[0].stopped);
    timers.advance(QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 2);
  });

  it("keeps going after a run throws", async () => {
    const { scheduler, timers, runs } = setup();
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    runs[0].fail(new Error("the runner broke"));
    await flush();
    assert.ok(!scheduler.running);
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 2);
  });

  it("stops everything when disposed: the run in flight, the wait, and later changes", async () => {
    const { scheduler, timers, runs } = setup();
    scheduler.signal();
    timers.advance(QUIET_MS);
    await flush();
    scheduler.signal();
    scheduler.dispose();
    assert.ok(runs[0].stopped);
    assert.ok(!scheduler.waiting);
    runs[0].finish();
    scheduler.signal();
    timers.advance(10 * QUIET_MS);
    await flush();
    assert.strictEqual(runs.length, 1);
  });
});

describe("RunSlot", () => {
  it("tells a listener when the run is cancelled, once", () => {
    const slot = new RunSlot();
    let told = 0;
    slot.onCancel(() => told++);
    slot.cancel();
    slot.cancel();
    assert.strictEqual(told, 1);
    assert.ok(slot.cancelled);
  });

  it("tells a listener that arrives late at once", () => {
    const slot = new RunSlot();
    slot.cancel();
    let told = 0;
    slot.onCancel(() => told++);
    assert.strictEqual(told, 1);
  });

  it("lets a listener stop listening", () => {
    const slot = new RunSlot();
    let told = 0;
    slot.onCancel(() => told++)();
    slot.cancel();
    assert.strictEqual(told, 0);
  });
});

describe("FailureOnce (#2269 DECISION D)", () => {
  it("shows a failure the first time only", () => {
    const once = new FailureOnce();
    assert.ok(once.first("bynk-not-found"));
    assert.ok(!once.first("bynk-not-found"));
    assert.ok(!once.first("bynk-not-found"));
  });

  it("keeps different failures apart", () => {
    const once = new FailureOnce();
    assert.ok(once.first("crash"));
    assert.ok(once.first("bynk-not-found"));
    assert.ok(!once.first("crash"));
  });

  it("shows a failure again after a run succeeds", () => {
    const once = new FailureOnce();
    assert.ok(once.first("crash"));
    assert.ok(!once.first("crash"));
    once.succeeded();
    assert.ok(once.first("crash"), "news again");
  });
});
