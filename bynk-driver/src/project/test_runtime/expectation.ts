class ExpectationError extends Error {
  location: string;
  start: number;
  end: number;
  constructor(location: string, start: number, end: number, detail: string) {
    super(`${detail}\n  at ${location}`);
    this.location = location;
    this.start = start;
    this.end = end;
  }
}
function __bynkExpectFailure(location: string, start: number, end: number, detail: string) {
  return new ExpectationError(location, start, end, detail);
}
function __bynkExpect(cond: boolean, location: string, start: number, end: number, detail: string): void {
  if (!cond) { throw __bynkExpectFailure(location, start, end, detail); }
}
// #2272: the declaration an invariant or transition violation names, as a failed case
// reports it. `undefined` for any other error, so it is omitted from the report.
function __bynkViolationOf(e: any): { kind: string, agent: string, name: string, location: string } | undefined {
  const v = e && e.invariantViolation;
  return v === undefined ? undefined : { kind: v.declaration, agent: v.agent, name: v.invariant, location: v.location };
}
function __bynkShow(v: unknown): string {
  try { return typeof v === "bigint" ? String(v) : (JSON.stringify(v) ?? String(v)); } catch { return String(v); }
}

