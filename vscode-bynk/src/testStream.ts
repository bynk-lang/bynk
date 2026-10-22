// Reads `bynk test --format ndjson` (#2267, Slice D stage 1): the line-buffered
// event reader and the assembler that folds the events back into the run
// document the Testing view already applies, handing over each suite as it ends.
//
// Dependency-free (no `vscode`), so the unit tests can drive it directly.
//
// The stream is one JSON event per line: `run-begin`, `suite-begin`, `case`
// (the document's case object behind `type` and `suite`), `suite-end`, then
// exactly one of `run-end` or `error`. A case a remote run skipped arrives as a
// `case` event with outcome `skipped` after the last `suite-end`. Unknown event
// types and fields are ignored, so the stream can grow additively.

import type { JsonCase, JsonError, JsonSuite, TestRun } from "./testPlan";

/** One event of the stream. Only `type` is relied on; the rest is read by name. */
export interface StreamEvent {
  type: string;
  [key: string]: unknown;
}

/** What the `bynk` in use is known to understand, learned from its refusals and
 *  kept until the extension is pointed at a different binary: the `ndjson` format
 *  (Slice D), and a repeatable `--case` with `--suite` (#2273). A driver that
 *  predates either rejects it on stderr, and the extension runs as it did before. */
export const driverSupport = { ndjson: true, sets: true };

/** Whether `stderr` is a driver refusing a repeated `--case` or `--suite` (one that
 *  predates the set filter): clap says the argument cannot be used twice, or does
 *  not know it. */
export function rejectsSets(stderr: string): boolean {
  return /cannot be used multiple times|unexpected argument '--suite'/.test(stderr);
}

/** Whether `stderr` is a driver refusing `--format ndjson` (one that predates the
 *  stream), which writes no events: clap names the value it did not accept. */
export function rejectsNdjson(stderr: string): boolean {
  return /ndjson/.test(stderr) && /invalid value|possible values|unexpected argument/i.test(stderr);
}

/** Splits text chunks into lines, across chunk boundaries, and parses each as an
 *  event. A line that is not a JSON object with a string `type` is dropped, as the
 *  driver drops a line of the runner's that is not an event. */
export class EventReader {
  private rest = "";

  /** The events completed by `chunk`. */
  push(chunk: string): StreamEvent[] {
    this.rest += chunk;
    const lines = this.rest.split("\n");
    this.rest = lines.pop() ?? "";
    return lines.flatMap(parseEvent);
  }

  /** The event in a final line that had no newline, once the stream has ended. */
  end(): StreamEvent[] {
    const last = this.rest;
    this.rest = "";
    return parseEvent(last);
  }
}

function parseEvent(line: string): StreamEvent[] {
  const text = line.trim();
  if (!text) return [];
  try {
    const value: unknown = JSON.parse(text);
    if (value && typeof value === "object" && typeof (value as { type?: unknown }).type === "string") {
      return [value as StreamEvent];
    }
  } catch {
    // Not an event.
  }
  return [];
}

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const num = (v: unknown): number | undefined => (typeof v === "number" ? v : undefined);

/** The case an event carries: the event is the document's case with `type` and
 *  `suite` in front, so dropping those leaves it. */
function caseOf(event: StreamEvent): JsonCase {
  const { type: _type, suite: _suite, ...rest } = event;
  void _type;
  void _suite;
  return rest as unknown as JsonCase;
}

/** Folds events into a {@link TestRun} and reports each suite when it is
 *  complete. */
export class RunAssembler {
  readonly doc: TestRun = { passed: 0, failed: 0, suites: [] };
  private current: JsonSuite | undefined;
  private terminal = false;
  private seen = 0;

  /** Apply one event. Returns the suite it completed, if any: a `suite-end`
   *  completes the suite it closes, and a skipped case that arrives after its
   *  suite ended is a suite of its own holding that case. */
  apply(event: StreamEvent): JsonSuite | undefined {
    const suites = this.doc.suites as JsonSuite[];
    this.seen += 1;
    switch (event.type) {
      case "suite-begin": {
        this.current = { name: str(event.name) ?? "", kind: str(event.kind) ?? "unit", cases: [] };
        suites.push(this.current);
        return undefined;
      }
      case "case": {
        const c = caseOf(event);
        if (c.outcome === "skipped") {
          this.doc.skipped = (this.doc.skipped ?? 0) + 1;
        } else if (c.outcome === "pass") {
          this.doc.passed += 1;
        } else {
          this.doc.failed += 1;
        }
        const name = str(event.suite);
        if (this.current && (name === undefined || name === this.current.name)) {
          this.current.cases.push(c);
          return undefined;
        }
        // Outside its suite's begin and end: a skipped case, which the runner
        // never saw. Report it on its own.
        const suite = suites.find((s) => s.name === name) ?? { name: name ?? "", kind: "unit", cases: [] };
        if (!suites.includes(suite)) suites.push(suite);
        suite.cases.push(c);
        return { name: suite.name, kind: suite.kind, cases: [c] };
      }
      case "suite-end": {
        const done = this.current;
        this.current = undefined;
        return done;
      }
      case "run-end": {
        this.terminal = true;
        this.doc.passed = num(event.passed) ?? this.doc.passed;
        this.doc.failed = num(event.failed) ?? this.doc.failed;
        const skipped = num(event.skipped);
        if (skipped !== undefined) this.doc.skipped = skipped;
        const transport = str(event.transport);
        if (transport !== undefined) this.doc.transport = transport;
        return undefined;
      }
      case "error": {
        this.terminal = true;
        const kind = event.kind === "compile" ? "compile" : "runtime";
        const error: JsonError = { kind };
        const message = str(event.message);
        if (message !== undefined) error.message = message;
        if (Array.isArray(event.diagnostics)) {
          error.diagnostics = event.diagnostics.filter((d): d is string => typeof d === "string");
        }
        const stderr = str(event.stderr);
        if (stderr !== undefined) error.stderr = stderr;
        this.doc.error = error;
        return undefined;
      }
      default:
        return undefined;
    }
  }

  /** The document once the stream has ended. A stream that ended with neither
   *  `run-end` nor `error` (a driver killed mid-run, or one that predates the
   *  `error` event) is a runtime failure, carrying the process's stderr. A suite
   *  left open is kept as the prefix the run reached. */
  finish(stderr: string): TestRun {
    if (!this.terminal) {
      const text = stderr.trim();
      this.doc.error = {
        kind: "runtime",
        message: "the test runner exited before completing",
        ...(text ? { stderr: text } : {}),
      };
    }
    return this.doc;
  }

  /** Whether any event has been applied. A stream with none came from a process
   *  that wrote no events: a rejected flag, or a failure before the run. */
  get hasEvents(): boolean {
    return this.seen > 0;
  }
}
