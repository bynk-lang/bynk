// #2093: the live views' refresh scheduling, model cache, target tracking and
// handler lookup — the `vscode`-free core of `viewRefresh.ts`/`livePanel.ts`.

import * as assert from "assert";

import {
  enclosingHandler,
  handlerKey,
  handlerSymbols,
  ModelCache,
  RefreshScheduler,
  shiftPosition,
  type SymbolLike,
  type Timers,
  toRefreshMode,
} from "../../src/refreshScheduler";

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

const OPTS = { typeDebounceMs: 500, settleMs: 1500 };

function setup(mode: "onSave" | "onType" | "manual") {
  const timers = new FakeTimers();
  const fires: string[][] = [];
  const s = new RefreshScheduler((keys) => fires.push([...keys].sort()), timers, OPTS);
  s.setMode(mode);
  return { s, timers, fires };
}

describe("RefreshScheduler (#2093 DECISION A)", () => {
  it("onSave: fires at once on save, ignores unsaved edits", () => {
    const { s, timers, fires } = setup("onSave");
    s.onEdit("a");
    timers.advance(2000);
    assert.deepStrictEqual(fires, []);
    s.onSave("a");
    assert.deepStrictEqual(fires, [["a"]]);
  });

  it("re-fires on a committed round while armed, and once more when it settles", () => {
    const { s, timers, fires } = setup("onSave");
    s.onSave("a");
    assert.ok(s.isArmed());
    s.onRoundCommitted();
    assert.strictEqual(fires.length, 2, "the round that sees the save re-fires");
    timers.advance(1500);
    assert.strictEqual(fires.length, 3, "the settle fallback fires last");
    assert.ok(!s.isArmed());
    s.onRoundCommitted();
    assert.strictEqual(fires.length, 3, "a round with nothing armed is ignored");
  });

  it("onType: debounces a burst of edits into one fire", () => {
    const { s, timers, fires } = setup("onType");
    s.onEdit("a");
    timers.advance(300);
    s.onEdit("b");
    timers.advance(300);
    assert.deepStrictEqual(fires, [], "still inside the debounce");
    timers.advance(200);
    assert.deepStrictEqual(fires, [["a", "b"]]);
  });

  // #2108 review: the settle timer of an earlier fire ran out while a later
  // edit's debounce was pending, cleared the touched keys and disarmed, so
  // the debounced fire found nothing to emit and the edit was lost.
  it("onType: an edit made just before the settle timer runs out still fires", () => {
    const { s, timers, fires } = setup("onType");
    s.onEdit("a");
    timers.advance(500);
    assert.strictEqual(fires.length, 1, "the first edit fires after the debounce");
    timers.advance(1300); // t = 1800
    s.onEdit("b");
    timers.advance(200); // t = 2000: the first fire's settle timer runs out
    const atSettle = fires.length;
    timers.advance(300); // t = 2300: the second edit's debounce ends
    assert.ok(fires.length > atSettle, "the second edit fires after its debounce");
    assert.ok(fires[fires.length - 1].includes("b"), "carrying the edited key");
    assert.ok(s.isArmed(), "and re-arms for the round that analyses it");
    timers.advance(1500);
    assert.ok(!s.isArmed(), "until its own settle timer runs out");
  });

  it("a save followed by typing within the settle window keeps the typed edit", () => {
    const { s, timers, fires } = setup("onType");
    s.onSave("a");
    timers.advance(1400);
    s.onEdit("b");
    timers.advance(100); // t = 1500: the save's settle timer runs out
    const atSettle = fires.length;
    timers.advance(400); // t = 1900
    assert.ok(fires.length > atSettle && fires[fires.length - 1].includes("b"));
  });

  it("manual: never fires on its own", () => {
    const { s, timers, fires } = setup("manual");
    s.onEdit("a");
    s.onSave("a");
    s.onRoundCommitted();
    timers.advance(5000);
    assert.deepStrictEqual(fires, []);
  });

  it("switching to manual drops a pending fire", () => {
    const { s, timers, fires } = setup("onType");
    s.onEdit("a");
    s.setMode("manual");
    timers.advance(5000);
    assert.deepStrictEqual(fires, []);
  });

  it("reads the setting, defaulting to onSave", () => {
    assert.strictEqual(toRefreshMode("onType"), "onType");
    assert.strictEqual(toRefreshMode("manual"), "manual");
    assert.strictEqual(toRefreshMode(undefined), "onSave");
    assert.strictEqual(toRefreshMode("sometimes"), "onSave");
  });
});

describe("ModelCache (#2093, shared with #2097)", () => {
  it("shares one load per key until invalidated", async () => {
    const cache = new ModelCache();
    let loads = 0;
    const load = async () => ++loads;
    assert.strictEqual(await cache.get("k", load), 1);
    assert.strictEqual(await cache.get("k", load), 1, "second consumer shares the fetch");
    cache.invalidate();
    assert.strictEqual(await cache.get("k", load), 2);
    assert.strictEqual(await cache.get("k", load, { fresh: true }), 3, "fresh bypasses");
    assert.strictEqual(await cache.get("k", load), 3, "and seeds the cache");
  });

  it("does not keep a failed load", async () => {
    const cache = new ModelCache();
    await assert.rejects(cache.get("k", () => Promise.reject(new Error("down"))));
    assert.strictEqual(await cache.get("k", async () => "ok"), "ok");
  });
});

const P = (line: number, character: number) => ({ line, character });
const change = (sl: number, sc: number, el: number, ec: number, text: string) => ({
  range: { start: P(sl, sc), end: P(el, ec) },
  text,
});

describe("shiftPosition (#2093 target tracking)", () => {
  it("moves down when lines are inserted above", () => {
    assert.deepStrictEqual(shiftPosition(P(10, 4), [change(2, 0, 2, 0, "a\nb\n")]), P(12, 4));
  });

  it("moves up when lines above are deleted", () => {
    assert.deepStrictEqual(shiftPosition(P(10, 4), [change(2, 0, 5, 0, "")]), P(7, 4));
  });

  it("ignores edits after the position", () => {
    assert.deepStrictEqual(shiftPosition(P(3, 2), [change(8, 0, 8, 0, "x\n")]), P(3, 2));
  });

  it("shifts the column for an edit earlier on the same line", () => {
    assert.deepStrictEqual(shiftPosition(P(3, 10), [change(3, 2, 3, 4, "abcd")]), P(3, 12));
  });

  it("collapses to the edit start when the position was replaced", () => {
    assert.deepStrictEqual(shiftPosition(P(5, 3), [change(4, 0, 6, 0, "z")]), P(4, 0));
  });
});

const R = (sl: number, el: number) => ({ start: P(sl, 0), end: P(el, 1) });
const sym = (name: string, kind: number, sl: number, el: number, children: SymbolLike[] = []): SymbolLike => ({
  name,
  kind,
  range: R(sl, el),
  selectionRange: R(sl, sl),
  children,
});

// Module(1) > Class(4) service > Method(5) handlers, plus a capability
// (Interface 10) whose Method children are not handlers.
const SYMBOLS: SymbolLike[] = [
  sym("shop", 1, 0, 30, [
    sym("Store", 10, 1, 4, [sym("get", 5, 2, 2)]),
    sym("api", 4, 6, 20, [sym("call", 5, 7, 9), sym("call", 5, 11, 13)]),
    sym("Counter", 4, 22, 29, [sym("count", 6, 23, 23), sym("call bump", 5, 24, 28)]),
  ]),
];

describe("handler symbols (#2093 cursor-follow, DECISION D lens)", () => {
  it("lists service and agent handlers, not capability methods", () => {
    const hs = handlerSymbols(SYMBOLS).map((h) => `${h.owner}.${h.symbol.name}@${h.symbol.range.start.line}`);
    assert.deepStrictEqual(hs, ["api.call@7", "api.call@11", "Counter.call bump@24"]);
  });

  it("finds the handler enclosing a position, and none outside one", () => {
    assert.strictEqual(enclosingHandler(SYMBOLS, P(12, 3))?.symbol.range.start.line, 11);
    assert.strictEqual(enclosingHandler(SYMBOLS, P(10, 0)), undefined, "between handlers");
    assert.strictEqual(enclosingHandler(SYMBOLS, P(2, 1)), undefined, "a capability method");
  });

  it("keys two same-named handlers apart", () => {
    const [a, b] = handlerSymbols(SYMBOLS);
    assert.notStrictEqual(handlerKey("file:///s.bynk", a), handlerKey("file:///s.bynk", b));
  });
});
