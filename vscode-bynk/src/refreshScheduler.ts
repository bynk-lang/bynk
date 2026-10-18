// #2093: when to re-pull a model view. Dependency-free (no `vscode` import) so
// the unit tests compile against it directly; `viewRefresh.ts` feeds it editor
// events and the language server's round signal.
//
// The four model requests (and the project tree, #2097) are served from the
// server's last *committed* analysis round (`committed_analysis` in
// `bynk-lsp/src/lib.rs`), which lags an edit by the server's diagnostics
// debounce. A re-pull fired the instant a file is saved can therefore return
// the pre-save model. So a trigger does two things: it fires (the round may
// already be current, as it is when the user paused before saving), and it
// *arms* the scheduler. While armed, a committed round (`onRoundCommitted`,
// driven by the server's diagnostics publishes) fires again. When the settle
// timer runs out it fires too, and it stays armed after that: the server's
// debounce is configurable and a large project's (or a slow machine's) round
// can land well after the settle timer. A late round that publishes
// diagnostics fires once more and disarms. But a clean project publishes no
// diagnostics at all (the server only publishes changes), so there may be no
// round signal to wait for: until then, follow-up fires back off over
// `followUpMs` (about 30 s in all), and the last one disarms (#2144). Repeat
// fires are cheap: the view drops a payload identical to the one it shows.

/** The `bynk.views.refresh` setting (#2093, DECISION A). */
export type RefreshMode = "onSave" | "onType" | "manual";

export const REFRESH_MODES: readonly RefreshMode[] = ["onSave", "onType", "manual"];

/** Read a setting value as a `RefreshMode`, defaulting to `onSave`. */
export function toRefreshMode(value: unknown): RefreshMode {
  return REFRESH_MODES.includes(value as RefreshMode) ? (value as RefreshMode) : "onSave";
}

/** The timer surface the scheduler needs — injectable so tests drive time. */
export interface Timers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export const realTimers: Timers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface SchedulerOptions {
  /** `onType` debounce after the last edit (the proposal's ~500 ms). */
  typeDebounceMs: number;
  /** How long a trigger stays armed for a committed round, and when the
   *  fallback fire lands. Longer than the server's default debounce. */
  settleMs: number;
  /** After the settle fire, the delays of the follow-up fires that catch up
   *  a round with no diagnostics publish to signal it; after the last one
   *  the scheduler disarms. */
  followUpMs: readonly number[];
}

export const DEFAULT_SCHEDULER_OPTIONS: SchedulerOptions = {
  typeDebounceMs: 500,
  settleMs: 1500,
  followUpMs: [2000, 4000, 8000, 16000],
};

/**
 * Coalesces edit/save triggers into refresh fires. Each fire carries the set
 * of keys (document URIs) touched since the trigger that armed it.
 */
export class RefreshScheduler {
  private mode: RefreshMode = "onSave";
  private touched = new Set<string>();
  private typeTimer: unknown;
  private settleTimer: unknown;
  private capTimer: unknown;
  private armed = false;
  /** The settle timer has run out: the next committed round is the last. */
  private settled = false;

  constructor(
    private readonly fire: (keys: ReadonlySet<string>) => void,
    private readonly timers: Timers = realTimers,
    private readonly options: SchedulerOptions = DEFAULT_SCHEDULER_OPTIONS,
  ) {}

  setMode(mode: RefreshMode): void {
    this.mode = mode;
    if (mode === "manual") this.reset();
  }

  getMode(): RefreshMode {
    return this.mode;
  }

  /** A document changed in the editor (unsaved). Only `onType` reacts. */
  onEdit(key: string): void {
    if (this.mode !== "onType") return;
    this.touched.add(key);
    if (this.typeTimer !== undefined) this.timers.clear(this.typeTimer);
    this.typeTimer = this.timers.set(() => {
      this.typeTimer = undefined;
      this.emit();
      this.arm();
    }, this.options.typeDebounceMs);
  }

  /** A document was saved. Every mode but `manual` reacts at once. */
  onSave(key: string): void {
    if (this.mode === "manual") return;
    this.touched.add(key);
    this.emit();
    this.arm();
  }

  /** The language server committed an analysis round. */
  onRoundCommitted(): void {
    if (!this.armed) return;
    this.emit();
    if (this.settled) this.disarm();
  }

  /** Whether a committed round would fire now (for tests and diagnostics). */
  isArmed(): boolean {
    return this.armed;
  }

  dispose(): void {
    this.reset();
  }

  private arm(): void {
    this.armed = true;
    this.settled = false;
    this.clearTimer("settleTimer");
    this.clearTimer("capTimer");
    this.settleTimer = this.timers.set(() => {
      this.settleTimer = undefined;
      this.emit();
      // An `onType` fire still pending owns the keys touched since: keep them
      // and stay armed (its own fire re-arms), or that edit would be lost.
      if (this.typeTimer !== undefined) return;
      // Stay armed for a round that lands late, but not forever.
      this.settled = true;
      this.followUp(0);
    }, this.options.settleMs);
  }

  /** Follow-up fire `i` (see `followUpMs`); the last one disarms. */
  private followUp(i: number): void {
    const delays = this.options.followUpMs;
    if (i >= delays.length) {
      this.disarm();
      return;
    }
    this.capTimer = this.timers.set(() => {
      this.capTimer = undefined;
      this.emit();
      this.followUp(i + 1);
    }, delays[i]);
  }

  private disarm(): void {
    this.clearTimer("settleTimer");
    this.clearTimer("capTimer");
    this.armed = false;
    this.settled = false;
    this.touched = new Set();
  }

  private clearTimer(name: "settleTimer" | "capTimer" | "typeTimer"): void {
    if (this[name] !== undefined) this.timers.clear(this[name]);
    this[name] = undefined;
  }

  private emit(): void {
    if (this.touched.size === 0) return;
    this.fire(new Set(this.touched));
  }

  private reset(): void {
    this.clearTimer("typeTimer");
    this.disarm();
  }
}

/**
 * One model fetch per key per generation, shared by every consumer. #2093's
 * architecture map and #2097's project tree both read `bynk/architectureModel`;
 * when a refresh fires, the first to ask issues the request and the other
 * awaits the same promise. `invalidate()` starts a new generation (called
 * before each refresh fires); `fresh` bypasses the cache for an explicit
 * command, whose result then seeds the cache.
 */
export class ModelCache {
  private entries = new Map<string, Promise<unknown>>();

  invalidate(): void {
    this.entries.clear();
  }

  get<T>(key: string, load: () => Promise<T>, opts: { fresh?: boolean } = {}): Promise<T> {
    if (!opts.fresh) {
      const hit = this.entries.get(key);
      if (hit) return hit as Promise<T>;
    }
    const p = load();
    this.entries.set(key, p);
    // A failed load must not stick: the next ask retries.
    p.catch(() => {
      if (this.entries.get(key) === p) this.entries.delete(key);
    });
    return p;
  }
}

/** A position in a document (structurally `vscode.Position`'s fields). */
export interface Pos {
  line: number;
  character: number;
}

/** A text edit as `onDidChangeTextDocument` reports it. */
export interface TextChange {
  range: { start: Pos; end: Pos };
  text: string;
}

function before(a: Pos, b: Pos): boolean {
  return a.line < b.line || (a.line === b.line && a.character <= b.character);
}

/**
 * Move a stored view target through edits, so a pinned or not-yet-retargeted
 * view keeps pointing at the same handler when lines are added above it.
 * Edits wholly after the position leave it alone; an edit that contains the
 * position collapses it to the edit's start. Changes are applied in the order
 * given (VS Code reports them so that each applies to the result of the last).
 */
export function shiftPosition(pos: Pos, changes: readonly TextChange[]): Pos {
  // Plain copies, never spreads: a `vscode.Position` keeps its fields as
  // private `_line`/`_character` behind getters, so `{ ...start }` would lose them.
  let p: Pos = { line: pos.line, character: pos.character };
  for (const c of changes) {
    const { start, end } = c.range;
    if (before(p, start)) continue; // edit is at or after the position
    if (!before(end, p)) {
      p = { line: start.line, character: start.character }; // the position was inside the replaced text
      continue;
    }
    const lines = c.text.split("\n");
    const addedLines = lines.length - 1;
    const removedLines = end.line - start.line;
    if (end.line === p.line) {
      // Same line as the edit's end: the column moves with the new text's tail.
      const tail = lines[lines.length - 1].length;
      const col = (addedLines === 0 ? start.character + tail : tail) + (p.character - end.character);
      p = { line: p.line + addedLines - removedLines, character: col };
    } else {
      p = { line: p.line + addedLines - removedLines, character: p.character };
    }
  }
  return p;
}

/** A document symbol, structurally (`vscode.DocumentSymbol`'s fields used). */
export interface SymbolLike {
  name: string;
  kind: number;
  range: { start: Pos; end: Pos };
  selectionRange: { start: Pos; end: Pos };
  children?: readonly SymbolLike[];
}

// `vscode.SymbolKind` values (stable LSP-derived numbering). `bynk-lsp`'s
// document symbols report a service or agent as `Class` and each of its
// handlers as a `Method` child (`bynk-lsp/src/document_symbols.rs`).
const SYMBOL_KIND_CLASS = 4;
const SYMBOL_KIND_METHOD = 5;

/** A handler the views can target: its symbol plus the owner's name. */
export interface HandlerSymbol {
  owner: string;
  symbol: SymbolLike;
}

function contains(range: { start: Pos; end: Pos }, pos: Pos): boolean {
  return before(range.start, pos) && before(pos, range.end);
}

/** Every service/agent handler in a document's symbol tree, in order. */
export function handlerSymbols(symbols: readonly SymbolLike[]): HandlerSymbol[] {
  const out: HandlerSymbol[] = [];
  const walk = (list: readonly SymbolLike[]) => {
    for (const s of list) {
      if (s.kind === SYMBOL_KIND_CLASS) {
        for (const c of s.children ?? []) {
          if (c.kind === SYMBOL_KIND_METHOD) out.push({ owner: s.name, symbol: c });
        }
      }
      if (s.children) walk(s.children);
    }
  };
  walk(symbols);
  return out;
}

/** The handler enclosing `pos`, if any. */
export function enclosingHandler(symbols: readonly SymbolLike[], pos: Pos): HandlerSymbol | undefined {
  return handlerSymbols(symbols).find((h) => contains(h.symbol.range, pos));
}

/** A stable identity for a handler within a document: owner, name and start
 *  line. Cursor-follow retargets only when this changes (#2093's thrash
 *  mitigation), not on every selection event inside one handler. */
export function handlerKey(uri: string, h: HandlerSymbol): string {
  return `${uri}#${h.owner}.${h.symbol.name}@${h.symbol.range.start.line}`;
}
