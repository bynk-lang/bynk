import { Ok, Err } from "./result.ts";
import type { Result } from "./result.ts";

// Concurrent effects, slice 0 (#2640): the join behind a `par { … }` block.
//
// Each effect of the block arrives as a thunk. `__bynkSettleAll` calls them in
// source order, so every effect is issued before any is awaited and before the
// next binding is evaluated, then waits for ALL of them to settle. It yields the
// values in source order, or raises the FIRST rejection in source order,
// whichever settled first in time. A thunk that throws synchronously (a check
// that fails before a promise exists) is a rejected entry like any other, so an
// earlier effect is never left unawaited. A fast failure therefore waits for a
// slow sibling: nothing the block started is still running when it returns or
// throws, so no work outlives the handler, and the fault a program sees does
// not depend on timing.
//
// Later rejections are attached to the raised one as `suppressed` (an array, in
// source order) so a log can show them, but they are not catchable separately.
// Not part of ADR 0310's published ABI: it is codegen.
export async function __bynkSettleAll<const T extends readonly (() => unknown)[]>(
  thunks: T,
): Promise<{ -readonly [P in keyof T]: Awaited<ReturnType<T[P]>> }> {
  const issued: Promise<unknown>[] = [];
  for (const thunk of thunks) {
    try {
      issued.push(Promise.resolve(thunk()));
    } catch (e) {
      issued.push(Promise.reject(e));
    }
  }
  const settled = await Promise.allSettled(issued);
  const failures: unknown[] = [];
  for (const r of settled) {
    if (r.status === "rejected") failures.push(r.reason);
  }
  if (failures.length > 0) {
    const [first, ...others] = failures;
    // The same reason raised twice must not become its own `suppressed`.
    const rest = others.filter((f) => f !== first);
    if (rest.length > 0 && typeof first === "object" && first !== null) {
      try {
        Object.defineProperty(first, "suppressed", {
          value: rest,
          configurable: true,
          writable: true,
        });
      } catch {
        // A frozen reason cannot carry them; the first rejection still stands.
      }
    }
    throw first;
  }
  return settled.map((r) => (r as { value: unknown }).value) as {
    -readonly [P in keyof T]: Awaited<ReturnType<T[P]>>;
  };
}

// Concurrent effects, slice 3 (#2683): `e.within(d)`.
//
// Races the effect against a deadline of `ms` milliseconds. If the effect
// settles first the timer is cleared and the call yields `Ok(value)` (an effect
// that faults first faults the call). If the deadline passes first the call
// yields `Err({ after: ms })` and the effect is handed, UNCANCELLED, to the
// execution context's `waitUntil`, so it runs to completion and its side effects
// still happen; a fault in it after that point is the platform's to log, not the
// handler's, which has already moved on. A tie goes to the effect. Nothing is
// cancelled (track Q7): `FetchOptions.timeout` is the tool when the work must
// stop.
//
// A deadline outside (0, 2^31 - 1] ms faults the call, like
// `FetchOptions.timeout`. The effect is already running by then, so it is handed
// to `waitUntil` first: no work outlives the handler unseen.
//
// `exec.timers`, when present, replaces the host's `setTimeout`: the test
// harness supplies virtual time through it. Not part of ADR 0310's published
// ABI: it is codegen.
export interface __BynkWithinExec {
  waitUntil(promise: Promise<unknown>): void;
  readonly timers?: {
    set(fn: () => void, ms: number): unknown;
    clear(handle: unknown): void;
  };
}
const __BYNK_TIMED_OUT: unique symbol = Symbol("bynk.timedOut");
export async function __bynkWithin<T>(
  promise: Promise<T>,
  ms: number,
  exec: __BynkWithinExec,
): Promise<Result<T, { readonly after: number }>> {
  if (!(ms > 0 && ms <= 2147483647)) {
    exec.waitUntil(promise);
    throw new Error(
      `\`within\` takes a Duration over zero and at most 2,147,483,647 ms, but got ${ms} ms`,
    );
  }
  const timers = exec.timers ?? {
    set: (fn: () => void, after: number): unknown => setTimeout(fn, after),
    clear: (handle: unknown): void => clearTimeout(handle as ReturnType<typeof setTimeout>),
  };
  let handle: unknown;
  const deadline = new Promise<typeof __BYNK_TIMED_OUT>((resolve) => {
    handle = timers.set(() => resolve(__BYNK_TIMED_OUT), ms);
  });
  let winner: T | typeof __BYNK_TIMED_OUT;
  try {
    winner = await Promise.race([promise, deadline]);
  } finally {
    timers.clear(handle);
  }
  if (winner === __BYNK_TIMED_OUT) {
    exec.waitUntil(promise);
    return Err({ after: ms });
  }
  return Ok(winner);
}

// Concurrent effects, slice 4 (#2711): `a.race(b)`.
//
// Resolves with the FIRST effect to succeed, in time; a tie (two effects that
// settle in the same turn) goes to the earlier one in source order, because each
// is subscribed in that order. A fast fault does not win: it waits to see whether
// a slower side succeeds. If EVERY effect faults, it raises the first fault in
// source order (not the first in time) with the rest attached as `suppressed`,
// as `__bynkSettleAll` does.
//
// Every effect that did not win is handed, UNCANCELLED, to the execution
// context's `waitUntil`, so it runs to completion and its side effects still
// happen; a fault in it is the platform's to log, not the handler's. Nothing is
// cancelled (track Q7): `race` is for idempotent work, and
// `FetchOptions.timeout` is the tool when the work must stop. A chain
// `a.race(b).race(c)` lowers to one call over `[a, b, c]`. Not part of ADR
// 0310's published ABI: it is codegen.
export function __bynkRace<T>(
  promises: readonly Promise<T>[],
  exec: { waitUntil(promise: Promise<unknown>): void },
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const faults: unknown[] = new Array(promises.length);
    let faulted = 0;
    let decided = false;
    promises.forEach((promise, i) => {
      Promise.resolve(promise).then(
        (value) => {
          if (decided) return;
          decided = true;
          // Settle first: a `waitUntil` that throws must not leave the race pending.
          resolve(value);
          promises.forEach((other, j) => {
            if (j === i) return;
            try {
              exec.waitUntil(other);
            } catch {
              // The race is decided; a host that refuses `waitUntil` cannot undo it.
            }
          });
        },
        (fault) => {
          faults[i] = fault;
          faulted += 1;
          if (decided || faulted < promises.length) return;
          decided = true;
          const [first, ...others] = faults;
          // The same reason raised twice must not become its own `suppressed`.
          const rest = others.filter((f) => f !== first);
          if (rest.length > 0 && typeof first === "object" && first !== null) {
            try {
              Object.defineProperty(first, "suppressed", {
                value: rest,
                configurable: true,
                writable: true,
              });
            } catch {
              // A frozen reason cannot carry them; the first fault still stands.
            }
          }
          reject(first);
        },
      );
    });
  });
}

// #3437: `e.detach()`, the compiler-inserted failure route for a detached effect.
//
// `detach` never starts anything: the effect is already running (effects are
// eager promises), so this hands the running promise to the execution context's
// `waitUntil` and returns at once. A detached effect has no caller to receive a
// failure, so the failure is reported here, by the runtime and not by `Logger`:
//
// - A fault (the promise rejects) is passed on UNCHANGED as a rejected promise
//   into `waitUntil`, the mechanism `__bynkReportFault` uses (#2192): the
//   platform records an exception and applies an uploaded source map. The
//   rejection is handled first, so a host that never awaits `waitUntil` sees no
//   unhandled rejection. The detach site is attached to an object reason as
//   `detachedAt`, never by wrapping it, so the stack stays the original one.
// - An `Err` (only when `errType` names the error type, i.e. the effect's `T` is
//   a `Result`) is one structured `console.error` record in the shape
//   `Logger.log` writes, carrying the site, the error's type name and, for a sum
//   error, its variant tag. Never the payload: it can hold a credential (#3437,
//   Decision D). The test harness's exec double supplies `report` to record it.
// - `Ok`, and every `T` that is not a `Result`, report nothing.
//
// Not part of ADR 0310's published ABI: it is codegen.
export interface __BynkDetachExec {
  waitUntil(promise: Promise<unknown>): void;
  readonly report?: (record: __BynkDetachReport) => void;
}
export interface __BynkDetachReport {
  readonly kind: "fault" | "err";
  readonly site: string;
  readonly type: string | null;
  readonly variant: string | null;
}
export function __bynkDetach(
  promise: Promise<unknown>,
  exec: __BynkDetachExec,
  site: string,
  errType?: string,
): Promise<void> {
  const watched = promise.then(
    (value) => {
      if (errType === undefined) return;
      const r = value as { readonly tag?: string; readonly error?: unknown } | null;
      if (r === null || typeof r !== "object" || r.tag !== "Err") return;
      const e = r.error as { readonly tag?: unknown } | null;
      const variant = typeof e === "object" && e !== null && typeof e.tag === "string" ? e.tag : null;
      const report: __BynkDetachReport = { kind: "err", site, type: errType, variant };
      if (exec.report !== undefined) {
        exec.report(report);
        return;
      }
      console.error({
        message: "a detached effect returned Err",
        level: "error",
        fields: { site, type: errType, variant: variant ?? "" },
      });
    },
    (reason: unknown) => {
      if (typeof reason === "object" && reason !== null) {
        try {
          Object.defineProperty(reason, "detachedAt", {
            value: site,
            configurable: true,
            writable: true,
          });
        } catch {
          // A frozen reason is passed on without the label.
        }
      }
      exec.report?.({ kind: "fault", site, type: null, variant: null });
      throw reason;
    },
  );
  watched.catch(() => {});
  exec.waitUntil(watched);
  return Promise.resolve();
}

