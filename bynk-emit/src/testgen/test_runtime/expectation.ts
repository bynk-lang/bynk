class ExpectationError extends Error {
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
function __bynkExpectFailure(location: string, start: number, end: number, detail: string, label?: string, was?: () => string) {
  const w = label === undefined ? { line: "" } : __bynkWasText(label, was);
  return new ExpectationError(location, start, end, detail + w.line, w.actual);
}
function __bynkExpect(cond: boolean, location: string, start: number, end: number, detail: string, label?: string, was?: () => string): void {
  if (!cond) { throw __bynkExpectFailure(location, start, end, detail, label, was); }
}
// #2272: the declaration an invariant or transition violation names, as a failed case
// reports it. `undefined` for any other error, so it is omitted from the report.
function __bynkViolationOf(e: any): { kind: string, agent: string, name: string, location: string } | undefined {
  const v = e && e.invariantViolation;
  return v === undefined ? undefined : { kind: v.declaration, agent: v.agent, name: v.invariant, location: v.location };
}
// #4035: `expect <call> violates <name>`. The claim holds when the call faulted with an
// `InvariantViolation` for the named declaration. `name` is `name`, `Agent.name` or `field.index`
// (an `@unique` constraint is named by its field and key), so a violation matches when the name
// is its own, or the agent's name before it. Anything else fails with what actually happened.
function __bynkViolates(e: any, name: string, location: string, start: number, end: number, src: string): boolean {
  const v = e && e.invariantViolation;
  if (v !== undefined && (v.invariant === name || `${v.agent}.${v.invariant}` === name)) return true;
  let what: string;
  if (v !== undefined) {
    const kind = v.declaration === "transition" ? "transition" : v.declaration === "unique" ? "unique constraint" : "invariant";
    what = `the commit was refused by ${kind} ${v.invariant}, not ${name}`;
  } else {
    const fault = e && typeof e === "object" && typeof e.kind === "string" ? e.kind : e instanceof Error ? (e.message === "" ? e.name : `${e.name}: ${e.message}`) : typeof e;
    what = `the call faulted with a different fault: ${fault}`;
  }
  throw new ExpectationError(location, start, end, `${src}\n  ${what}`);
}
function __bynkViolatesNone(location: string, start: number, end: number, src: string): never {
  throw new ExpectationError(location, start, end, `${src}\n  the call returned without faulting`);
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
    // #4120: a set and a map print as the `Set.of` and `Map.of` that builds them.
    if (v instanceof Set) return "Set.of([" + [...v].map((e) => __bynkShow(e)).join(",") + "])";
    if (v instanceof Map) {
      return "Map.of([" + [...v].map(([k, x]) => "{\"key\":" + __bynkShow(k) + ",\"value\":" + __bynkShow(x) + "}").join(",") + "])";
    }
    if (v !== null && typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype) {
      // An `Option` prints as it is written.
      const keys = Object.keys(v);
      if ((v as any).tag === "None" && keys.length === 1) return "None";
      if ((v as any).tag === "Some" && keys.length === 2 && keys.includes("value")) return "Some(" + __bynkShow((v as any).value) + ")";
      return "{" + Object.keys(v).map((k) => JSON.stringify(k) + ":" + __bynkShow((v as any)[k])).join(",") + "}";
    }
    return JSON.stringify(v) ?? String(v);
  } catch { return String(v); }
}

// #4229: a `Duration` as the largest unit literal that holds it exactly (`3.seconds`,
// `1.hour`, `1500.milliseconds`), an integral count only; anything else prints as the number.
function __bynkShowDuration(ms: number): string {
  if (!Number.isInteger(ms)) return String(ms);
  const units: Array<[number, string, string]> = [
    [86400000, "day", "days"],
    [3600000, "hour", "hours"],
    [60000, "minute", "minutes"],
    [1000, "second", "seconds"],
  ];
  if (ms === 0) return "0.seconds";
  const abs = Math.abs(ms);
  for (const [size, one, many] of units) {
    if (abs % size === 0) {
      const n = ms / size;
      return `${n}.${Math.abs(n) === 1 ? one : many}`;
    }
  }
  return `${ms}.${abs === 1 ? "millisecond" : "milliseconds"}`;
}
function __bynkShowFields(v: any, shapes: Record<string, any>): string {
  return "{" + Object.keys(v).map((k) => JSON.stringify(k) + ":" + (shapes[k] != null ? __bynkShowTyped(v[k], shapes[k]) : __bynkShow(v[k]))).join(",") + "}";
}
// A value printed beside the shape of the `Duration`s and `Instant`s in its type
// (`bynk-emit/src/testgen/show_shape.rs`): `"D"`, `"I"`, `["l", s]`, `["o", s]`,
// `["res", ok, err]`, `["r", { f: s }]`, `["s", { Variant: { f: s } }]`.
function __bynkShowTyped(v: unknown, shape: any): string {
  try {
    if (shape === "D" && typeof v === "number") return __bynkShowDuration(v);
    if (shape === "I" && typeof v === "number") return `Instant.fromEpochMillis(${v})`;
    if (Array.isArray(shape)) {
      const obj = v as any;
      switch (shape[0]) {
        case "l": if (Array.isArray(v)) return "[" + v.map((e) => __bynkShowTyped(e, shape[1])).join(",") + "]"; break;
        case "o":
          if (obj !== null && typeof obj === "object" && obj.tag === "None") return "None";
          if (obj !== null && typeof obj === "object" && obj.tag === "Some") return "Some(" + __bynkShowTyped(obj.value, shape[1]) + ")";
          break;
        case "res": if (obj !== null && typeof obj === "object") return __bynkShowFields(obj, { value: shape[1], error: shape[2] }); break;
        case "r": if (obj !== null && typeof obj === "object") return __bynkShowFields(obj, shape[1]); break;
        case "s": if (obj !== null && typeof obj === "object") return __bynkShowFields(obj, shape[1][obj.tag] ?? {}); break;
      }
    }
  } catch { /* fall through to the type-blind printer */ }
  return __bynkShow(v);
}
