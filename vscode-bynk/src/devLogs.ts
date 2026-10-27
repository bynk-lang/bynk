// The dependency-free core of the Bynk Logs view (#2474): the bounded buffer of
// worker log lines `bynk dev --log-events` reports, the filter over them, and
// how an entry is rendered. No `vscode` import, so the unit tests compile
// against it directly (see tsconfig.test.json); `devLogsView.ts` wires it to
// the editor.
//
// The driver has already rewritten stack frames into generated code to `.bynk`
// locations (`/abs/path/src/shop.bynk:12:3`); the view shows them as the
// driver sent them, and the editor's output channel makes an absolute
// `path:line:col` clickable.

import type { LogLevel } from "./devEvents";

/** One worker log line. `context` is the worker dir that printed it, or
 *  `bynk dev` for the view's own session markers. */
export interface LogEntry {
  context: string;
  level: LogLevel;
  text: string;
}

/** How many lines the view keeps; the oldest go first. Nothing is written to
 *  disk, so a closed window forgets them (worker logs can carry secrets). */
export const MAX_LOG_ENTRIES = 5000;

const RANK: Record<LogLevel, number> = { info: 0, warn: 1, error: 2 };

export interface LogFilter {
  /** Show at least this level. */
  minLevel: LogLevel;
  /** Show only these contexts; `undefined` shows every one. */
  contexts?: ReadonlySet<string>;
}

export const NO_FILTER: LogFilter = { minLevel: "info" };

export function passes(filter: LogFilter, entry: LogEntry): boolean {
  if (RANK[entry.level] < RANK[filter.minLevel]) return false;
  return !filter.contexts || filter.contexts.has(entry.context);
}

/** A bounded buffer: appending past `capacity` drops the oldest entry. */
export class LogBuffer {
  private entries: LogEntry[] = [];

  constructor(readonly capacity = MAX_LOG_ENTRIES) {}

  push(entry: LogEntry): void {
    this.entries.push(entry);
    // Dropped in blocks, so a busy worker does not shift the array per line.
    if (this.entries.length > this.capacity + Math.ceil(this.capacity / 10)) {
      this.entries.splice(0, this.entries.length - this.capacity);
    }
  }

  clear(): void {
    this.entries = [];
  }

  get size(): number {
    return Math.min(this.entries.length, this.capacity);
  }

  /** The kept entries, oldest first, that pass `filter`. */
  view(filter: LogFilter = NO_FILTER): LogEntry[] {
    return this.entries.slice(-this.capacity).filter((e) => passes(filter, e));
  }

  /** The contexts seen so far, sorted. */
  contexts(): string[] {
    return [...new Set(this.entries.map((e) => e.context))].sort();
  }
}

/** The line an entry is shown as. A stack frame (an indented `at …`) keeps its
 *  indent under the context tag, so a trace reads as one block. */
export function renderEntry(entry: LogEntry, showContext: boolean): string {
  return showContext ? `[${entry.context}] ${entry.text}` : entry.text;
}

/** A short description of a filter, for the view's status line. */
export function describeFilter(filter: LogFilter): string {
  const parts: string[] = [];
  if (filter.minLevel !== "info") parts.push(`${filter.minLevel} and above`);
  if (filter.contexts) parts.push([...filter.contexts].sort().join(", ") || "no context");
  return parts.length > 0 ? parts.join("; ") : "all logs";
}
