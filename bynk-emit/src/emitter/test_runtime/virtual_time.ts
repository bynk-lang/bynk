let __bynkClockNow = 0;
interface __BynkTimer {
  at: number;
  effect: boolean;
  seq: number;
  fire: () => void;
}
let __bynkTimerQueue: __BynkTimer[] = [];
let __bynkTimerSeq = 0;
let __bynkInflight = 0;
let __bynkVirtual = 0;
const __bynkCounted = new globalThis.WeakSet<object>();
function __bynkResetVirtualTime(): void {
  __bynkClockNow = 0;
  __bynkTimerQueue = [];
  __bynkTimerSeq = 0;
  __bynkInflight = 0;
  __bynkVirtual = 0;
}
function __bynkTakeTimer(upTo: number): __BynkTimer | undefined {
  let best = -1;
  for (let i = 0; i < __bynkTimerQueue.length; i++) {
    const t = __bynkTimerQueue[i]!;
    if (t.at > upTo) continue;
    if (best < 0) {
      best = i;
      continue;
    }
    const b = __bynkTimerQueue[best]!;
    if (t.at !== b.at ? t.at < b.at : t.effect !== b.effect ? t.effect : t.seq < b.seq) best = i;
  }
  return best < 0 ? undefined : __bynkTimerQueue.splice(best, 1)[0];
}
function __bynkAdvanceClock(by: number): void {
  if (!(by >= 0)) throw new globalThis.Error(`\`advance Clock by\` takes a non-negative Duration, but got ${by} ms`);
  const target = __bynkClockNow + by;
  for (let t = __bynkTakeTimer(target); t !== undefined; t = __bynkTakeTimer(target)) {
    __bynkClockNow = globalThis.Math.max(__bynkClockNow, t.at);
    t.fire();
  }
  __bynkClockNow = target;
}
const __bynkClock = { async now(): Promise<number> { return __bynkClockNow; } };
const __bynkTimers = {
  set(fn: () => void, ms: number): unknown {
    const t: __BynkTimer = { at: __bynkClockNow + ms, effect: false, seq: __bynkTimerSeq++, fire: fn };
    __bynkTimerQueue.push(t);
    return t;
  },
  clear(handle: unknown): void {
    const i = __bynkTimerQueue.indexOf(handle as __BynkTimer);
    if (i >= 0) __bynkTimerQueue.splice(i, 1);
  },
};
function __bynkSleep(ms: number): Promise<void> {
  if (!(ms >= 0)) throw new globalThis.Error(`\`stub … after\` takes a non-negative Duration, but got ${ms} ms`);
  __bynkVirtual++;
  return new globalThis.Promise<void>((resolve) => {
    __bynkTimerQueue.push({
      at: __bynkClockNow + ms,
      effect: true,
      seq: __bynkTimerSeq++,
      fire: () => {
        __bynkVirtual--;
        resolve();
      },
    });
  });
}
function __bynkNever(): Promise<never> {
  __bynkVirtual++;
  return new globalThis.Promise<never>(() => {});
}
function __bynkTrackDeps(deps: any): void {
  __bynkResetVirtualTime();
  for (const cap of globalThis.Object.keys(deps)) {
    const impl = deps[cap];
    if (cap.startsWith("__") || impl === null || typeof impl !== "object") continue;
    deps[cap] = new globalThis.Proxy(impl, {
      get(target, prop) {
        const v = (target as any)[prop];
        if (typeof v !== "function") return v;
        return (...args: any[]) => {
          const r = v.apply(target, args);
          // A wrapper the recording proxy lays over this one returns the very
          // same promise, which must not be counted twice.
          if (r !== null && typeof r === "object" && typeof r.then === "function" && !__bynkCounted.has(r)) {
            __bynkCounted.add(r);
            __bynkInflight++;
            const done = () => {
              __bynkInflight--;
            };
            r.then(done, done);
          }
          return r;
        };
      },
    });
  }
}
async function __bynkDriveUntil(done: () => boolean, stuckThrows: boolean): Promise<void> {
  for (;;) {
    await new globalThis.Promise<void>((resolve) => globalThis.setTimeout(resolve, 0));
    if (done()) return;
    // Every capability call in flight is waiting on virtual time. A call can wait
    // on several stubs at once (a provider operation in a `par` block), so the
    // sleepers may outnumber the calls, never the other way round.
    if (__bynkInflight > 0 && __bynkVirtual >= __bynkInflight) {
      const t = __bynkTakeTimer(globalThis.Infinity);
      if (t === undefined) {
        if (stuckThrows) throw new globalThis.Error("bynk: the call can never complete: every capability call in flight is a `stub … never`, and no timer is pending");
        return;
      }
      __bynkClockNow = globalThis.Math.max(__bynkClockNow, t.at);
      t.fire();
    }
  }
}
async function __bynkSettleVirtual<T>(deps: { __exec?: { pending: Promise<unknown>[] } }, call: Promise<T>): Promise<T> {
  const outcome = { finished: false, ok: false, value: undefined as T | undefined, error: undefined as unknown };
  call.then(
    (value) => {
      outcome.finished = true;
      outcome.ok = true;
      outcome.value = value;
    },
    (error) => {
      outcome.finished = true;
      outcome.error = error;
    },
  );
  await __bynkDriveUntil(() => outcome.finished, true);
  const pending = deps.__exec?.pending;
  while (pending !== undefined && pending.length > 0) {
    const batch = pending.splice(0);
    let settled = 0;
    for (const p of batch) {
      p.then(() => settled++, () => settled++);
    }
    await __bynkDriveUntil(() => settled === batch.length, false);
  }
  if (!outcome.ok) throw outcome.error;
  return outcome.value as T;
}
