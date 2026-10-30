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

// Standard-library K6b (#2598): the `Int` division companions and `pow`. Each
// faults where `/` does (a zero divisor) and where a result would leave the
// safe-integer domain, so they live here for the same coverage reason.

function nonZero(r: number, op: string): void {
  if (r === 0) throw new Error(`Int.${op}: division by zero`);
}

// The remainder of `/` (which truncates): the dividend's sign, so
// `a == b * (a / b) + a.rem(b)`. `+ 0` turns a `-0` into `0`.
export function __bynkIntRem(l: number, r: number): number {
  nonZero(r, "rem");
  return (l % r) + 0;
}

// Floor division: rounds toward negative infinity. `+ 0` turns `0.div(-5)`'s
// `-0` into `0`, as `rem` and `mod` do.
export function __bynkIntDivFloor(l: number, r: number): number {
  nonZero(r, "div");
  return Math.floor(l / r) + 0;
}

// The remainder of floor division: the divisor's sign, so
// `a == b * a.div(b) + a.mod(b)` and `x.mod(n)` is in `0..n-1` for `n > 0`.
// `r` is added only when the remainder's sign differs from the divisor's, so
// the result stays within `(-|r|, |r|)`: `((l % r) + r) % r` would pass 2^53
// for a large `r` and round before the second `%`.
export function __bynkIntMod(l: number, r: number): number {
  nonZero(r, "mod");
  const m = l % r;
  return (m !== 0 && m < 0 !== r < 0 ? m + r : m) + 0;
}

// An exact power by repeated squaring, checking each step stays a safe
// integer (`Math.pow` loses precision before it overflows). A negative
// exponent has no `Int` result; `0.pow(0)` is 1.
export function __bynkIntPow(base: number, exp: number): number {
  if (exp < 0) throw new Error(`Int.pow: a negative exponent (${exp}) has no Int result`);
  let result = 1;
  let b = base;
  let e = exp;
  const check = (v: number): number => {
    if (!Number.isSafeInteger(v)) throw new Error(`Int.pow: ${base}^${exp} is past the safe Int range`);
    return v;
  };
  while (e > 0) {
    if (e % 2 === 1) result = check(result * b);
    e = Math.floor(e / 2);
    if (e > 0) b = check(b * b);
  }
  return result;
}

