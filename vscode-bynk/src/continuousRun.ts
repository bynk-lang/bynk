// #2269 (Test Explorer depth, Slice E): the Watch toggle's rerun policy.
// Dependency-free (no `vscode` import) so the unit tests compile against it
// directly; `testing.ts` feeds it the change signals of `viewRefresh.ts` and
// supplies the run it starts.
//
// A change to a model source waits out a quiet period, so a burst of saves
// (a format-on-save, a branch checkout) is one rerun, not many. A change that
// arrives while a run is in flight cancels it at once, since that run is
// measuring code that no longer exists, and the next run starts only after the
// cancelled one has ended: runs never overlap, and a superseded run's results
// are never shown over a newer one's.

import { realTimers, type Timers } from "./refreshScheduler";

/** How long after the last change signal a rerun starts (DECISION A). A constant
 *  rather than a setting: a rerun is a full compile, so a longer wait only delays
 *  the answer, and a shorter one only cancels runs that had barely begun. */
export const QUIET_MS = 1000;

/** The hand-over a run gets: whether it has been told to stop, and a way to hear
 *  of it. The run owns turning that into killing its process. */
export class RunSlot {
  private stopped = false;
  private readonly listeners = new Set<() => void>();

  get cancelled(): boolean {
    return this.stopped;
  }

  /** Call `fn` when the run is cancelled (at once, if it already was). Returns a
   *  function that stops listening. */
  onCancel(fn: () => void): () => void {
    if (this.stopped) {
      fn();
      return () => undefined;
    }
    this.listeners.add(fn);
    return () => void this.listeners.delete(fn);
  }

  cancel(): void {
    if (this.stopped) return;
    this.stopped = true;
    const listeners = [...this.listeners];
    this.listeners.clear();
    for (const fn of listeners) fn();
  }
}

/** Turns change signals into runs. `start` runs the watched tests and settles when
 *  the run has ended; it reports its own failures, so one that throws is dropped
 *  here rather than stopping the session. */
export class ContinuousScheduler {
  private quiet: unknown;
  private current: { slot: RunSlot; done: Promise<void> } | undefined;
  /** Bumped by every signal, so a launch that was waiting for a cancelled run to end
   *  knows a newer change has overtaken it and leaves the start to that change. */
  private generation = 0;
  private disposed = false;

  constructor(
    private readonly start: (slot: RunSlot) => Promise<void>,
    private readonly timers: Timers = realTimers,
    private readonly quietMs: number = QUIET_MS,
  ) {}

  /** A watched source changed (a save, or a change made outside the editor). */
  signal(): void {
    if (this.disposed) return;
    this.generation++;
    // DECISION B: the run in flight is measuring stale code.
    this.current?.slot.cancel();
    if (this.quiet !== undefined) this.timers.clear(this.quiet);
    const ticket = this.generation;
    this.quiet = this.timers.set(() => {
      this.quiet = undefined;
      void this.launch(ticket);
    }, this.quietMs);
  }

  /** Whether a run is in flight (a cancelled one still winding down counts). */
  get running(): boolean {
    return this.current !== undefined;
  }

  /** Whether a change is waiting out its quiet period. */
  get waiting(): boolean {
    return this.quiet !== undefined;
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.quiet !== undefined) this.timers.clear(this.quiet);
    this.quiet = undefined;
    this.current?.slot.cancel();
  }

  private async launch(ticket: number): Promise<void> {
    // A cancelled run may still be ending (its process is being killed); the next
    // one waits for it.
    await this.current?.done;
    if (this.disposed || ticket !== this.generation) return;
    const slot = new RunSlot();
    const done = (async () => {
      try {
        await this.start(slot);
      } catch {
        // The run reported its own failure.
      } finally {
        if (this.current?.slot === slot) this.current = undefined;
      }
    })();
    this.current = { slot, done };
  }
}

/** DECISION D: a continuous session shows a given failure (a missing `bynk`, a
 *  crashed runner) once, then stays quiet until a run succeeds; repeating a
 *  notification on every save would be hostile. The failure is still written to the
 *  run's output each time. */
export class FailureOnce {
  private readonly shown = new Set<string>();

  /** Whether to tell the user about `kind` now: the first time only. */
  first(kind: string): boolean {
    if (this.shown.has(kind)) return false;
    this.shown.add(kind);
    return true;
  }

  /** A run succeeded: the next failure is news again. */
  succeeded(): void {
    this.shown.clear();
  }
}
