export class ExpectationError extends Error {
  location: string;
  start: number;
  end: number;
  actual?: string;
  constructor(location: string, start: number, end: number, detail: string, actual?: string) {
    super(`${detail}\n  at ${location}`);
    this.location = location;
    this.start = start;
    this.end = end;
    this.actual = actual;
  }
}
// #3980: the value a failed `expect <subject> is …` (or `expect false` in a `match` arm) tested.
// `was` prints it with `__bynkShow`; it runs only on failure, and a throw while
// printing leaves the report as it was. The message shows at most 2,000 characters; `actual`
// (the `--format json` record) carries the whole text.
function __bynkWasText(label: string, was: (() => string) | undefined): { line: string, actual?: string } {
  if (was === undefined) return { line: "" };
  let full: string;
  try { full = was(); } catch { return { line: "" }; }
  const shown = full.length > 2000 ? `${full.slice(0, 2000)}… (${full.length - 2000} more characters; \`--format json\` has the whole value)` : full;
  return { line: `\n  ${label} was ${shown}`, actual: full };
}
export function __bynkExpectFailure(location: string, start: number, end: number, detail: string, label?: string, was?: () => string) {
  const w = label === undefined ? { line: "" } : __bynkWasText(label, was);
  return new ExpectationError(location, start, end, detail + w.line, w.actual);
}
export function __bynkExpect(cond: boolean, location: string, start: number, end: number, detail: string, label?: string, was?: () => string): void {
  if (!cond) { throw __bynkExpectFailure(location, start, end, detail, label, was); }
}
// #2272: the declaration an invariant or transition violation names, as a failed case
// reports it. `undefined` for any other error, so it is omitted from the report.
export function __bynkViolationOf(e: any): { kind: string, agent: string, name: string, location: string } | undefined {
  const v = e && e.invariantViolation;
  return v === undefined ? undefined : { kind: v.declaration, agent: v.agent, name: v.invariant, location: v.location };
}
export function __bynkShow(v: unknown): string {
  try { return typeof v === "bigint" ? String(v) : (JSON.stringify(v) ?? String(v)); } catch { return String(v); }
}

