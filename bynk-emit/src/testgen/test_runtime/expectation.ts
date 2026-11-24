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
// #4198: a `Decimal` (`{ coefficient: bigint, scale }`) and a `Bytes` (`Uint8Array`) print in
// Bynk's own form, wherever they sit, instead of throwing in `JSON.stringify` or printing
// as a bare object. `Duration` and `Instant` are plain numbers at runtime.
function __bynkShowLeaf(x: any): unknown {
  if (x !== null && typeof x === "object" && typeof x.coefficient === "bigint" && typeof x.scale === "number") {
    const neg = x.coefficient < 0n;
    let digits = (neg ? -x.coefficient : x.coefficient).toString();
    let text = digits;
    if (x.scale > 0) {
      if (digits.length <= x.scale) digits = "0".repeat(x.scale + 1 - digits.length) + digits;
      text = digits.slice(0, digits.length - x.scale) + "." + digits.slice(digits.length - x.scale);
    }
    return (neg ? "-" : "") + text + "d";
  }
  if (x instanceof Uint8Array) return `Bytes.fromHex("${Array.from(x, (b) => b.toString(16).padStart(2, "0")).join("")}")`;
  return x;
}
function __bynkShow(v: unknown): string {
  try {
    if (typeof v === "bigint") return String(v);
    const leaf = __bynkShowLeaf(v);
    if (typeof leaf === "string" && leaf !== v) return leaf;
    if (Array.isArray(v)) return "[" + v.map((e) => __bynkShow(e)).join(",") + "]";
    if (v !== null && typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype) {
      return "{" + Object.keys(v).map((k) => JSON.stringify(k) + ":" + __bynkShow((v as any)[k])).join(",") + "}";
    }
    return JSON.stringify(v) ?? String(v);
  } catch { return String(v); }
}

