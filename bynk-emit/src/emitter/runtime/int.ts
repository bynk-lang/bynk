// #1657 (runtime-semantics track §3.4): an `Int` is a JS safe integer,
// ±(2^53 − 1). These are the two in-language operations that would otherwise
// produce an `Int` outside that domain; each faults instead.
//
// They live here rather than inline in generated code so that `bynkc test
// --coverage`, which attributes only `.bynk` source, does not report the
// never-taken fault branch on every division or conversion line.

// `Int` division truncates toward zero; division by zero is a runtime fault
// rather than `Infinity`/`NaN` as an `Int`.
export function __bynkIntDiv(l: number, r: number): number {
  if (r === 0) throw new Error("Int division by zero");
  return Math.trunc(l / r);
}

// A Float→`Int` conversion (`round`/`floor`/`ceil`/`truncate`): the converted
// value must be a safe integer, which rules out a non-finite `Float` and one
// past ±2^53.
export function __bynkToInt(converted: number, op: string): number {
  if (!Number.isSafeInteger(converted)) {
    throw new Error(`Float.${op}: result is not a safe Int`);
  }
  return converted;
}

