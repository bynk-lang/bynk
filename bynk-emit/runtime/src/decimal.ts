import type { Option } from "./result.ts";
import { Some, None } from "./result.ts";

// #2622: the `Decimal` primitive runtime. A `Decimal` is an exact base-10
// number: an arbitrary-precision integer `coefficient` (a `bigint`) and a
// non-negative integer `scale`, the value being `coefficient / 10^scale`. It is
// a frozen plain object, so a record holding one is still structurally plain;
// the `bigint` coefficient is what tells a decimal apart from every other
// Bynk value (an `Int` is a `number`, never a `bigint`).
//
// The scale is *kept* as written (`1.20` has scale 2, `1.2` scale 1) because
// `scale()` and `toString()` expose it, but equality and order are by numeric
// value (`1.20 == 1.2`, DECISION E). `+`, `-` and `*` are exact; `/` does not
// exist - `div` states the scale and the rounding mode.
//
// Everything here is internal, not part of the published ABI (ADR 0310). The
// wire form is a JSON string (`"1.20"`); `JSON.stringify` throws on a `bigint`,
// so a decimal always crosses a boundary through `__bynkDecimalToString`.

export type __BynkDecimalValue = {
  readonly coefficient: bigint;
  readonly scale: number;
};

/** The rounding modes `round`/`div` accept (`RoundingMode.<mode>`). */
export type __BynkRoundingMode =
  | "Down"
  | "Up"
  | "Floor"
  | "Ceiling"
  | "HalfUp"
  | "HalfDown"
  | "HalfEven";

/** The longest decimal text `__bynkDecimalParse` accepts, in characters. */
const MAX_TEXT = 1000;

/** The largest scale `round` and `div` accept; above it they fault, as a negative scale does. */
export const __BYNK_DECIMAL_MAX_SCALE = 1000;

function checkScale(scale: number, op: string): void {
  if (!Number.isSafeInteger(scale) || scale < 0 || scale > __BYNK_DECIMAL_MAX_SCALE) {
    throw new Error(`Decimal.${op}: the scale must be an Int from 0 to ${__BYNK_DECIMAL_MAX_SCALE}`);
  }
}

/** Build a decimal from a coefficient and a scale. */
export function __bynkDecimal(coefficient: bigint, scale: number): __BynkDecimalValue {
  return Object.freeze({ coefficient, scale });
}

/** Whether `v` is a decimal (the `bigint` coefficient is the discriminator). */
export function __bynkIsDecimal(v: unknown): v is __BynkDecimalValue {
  return (
    typeof v === "object" &&
    v !== null &&
    typeof (v as { coefficient?: unknown }).coefficient === "bigint"
  );
}

const TEN = 10n;

function pow10(n: number): bigint {
  return TEN ** BigInt(n);
}

// Raise `d`'s coefficient to scale `to` (>= d.scale).
function lift(d: __BynkDecimalValue, to: number): bigint {
  return to === d.scale ? d.coefficient : d.coefficient * pow10(to - d.scale);
}

export function __bynkDecimalAdd(a: __BynkDecimalValue, b: __BynkDecimalValue): __BynkDecimalValue {
  const s = Math.max(a.scale, b.scale);
  return __bynkDecimal(lift(a, s) + lift(b, s), s);
}

export function __bynkDecimalSub(a: __BynkDecimalValue, b: __BynkDecimalValue): __BynkDecimalValue {
  const s = Math.max(a.scale, b.scale);
  return __bynkDecimal(lift(a, s) - lift(b, s), s);
}

export function __bynkDecimalMul(a: __BynkDecimalValue, b: __BynkDecimalValue): __BynkDecimalValue {
  return __bynkDecimal(a.coefficient * b.coefficient, a.scale + b.scale);
}

export function __bynkDecimalNeg(a: __BynkDecimalValue): __BynkDecimalValue {
  return __bynkDecimal(-a.coefficient, a.scale);
}

export function __bynkDecimalAbs(a: __BynkDecimalValue): __BynkDecimalValue {
  return a.coefficient < 0n ? __bynkDecimal(-a.coefficient, a.scale) : a;
}

/** The numeric order: -1, 0 or 1, ignoring how each side is scaled. */
export function __bynkDecimalCmp(a: __BynkDecimalValue, b: __BynkDecimalValue): number {
  const s = Math.max(a.scale, b.scale);
  const x = lift(a, s);
  const y = lift(b, s);
  return x < y ? -1 : x > y ? 1 : 0;
}

/** Numeric equality: `1.20 == 1.2`. */
export function __bynkDecimalEqual(a: __BynkDecimalValue, b: __BynkDecimalValue): boolean {
  return __bynkDecimalCmp(a, b) === 0;
}

export function __bynkDecimalMin(a: __BynkDecimalValue, b: __BynkDecimalValue): __BynkDecimalValue {
  return __bynkDecimalCmp(b, a) < 0 ? b : a;
}

export function __bynkDecimalMax(a: __BynkDecimalValue, b: __BynkDecimalValue): __BynkDecimalValue {
  return __bynkDecimalCmp(b, a) > 0 ? b : a;
}

// Divide `n` by the positive `d`, rounding the quotient by `mode`. `sign` is the
// sign of the true quotient (-1 or 1) so a tie or a directed mode knows which
// way "up" is. `n` and `d` are magnitudes (non-negative).
function divRound(n: bigint, d: bigint, negative: boolean, mode: __BynkRoundingMode): bigint {
  const q = n / d;
  const r = n % d;
  if (r === 0n) return negative ? -q : q;
  // Whether to bump the magnitude by one.
  let up: boolean;
  switch (mode) {
    case "Down":
      up = false;
      break;
    case "Up":
      up = true;
      break;
    case "Floor":
      up = negative;
      break;
    case "Ceiling":
      up = !negative;
      break;
    default: {
      const twice = r * 2n;
      if (twice > d) up = true;
      else if (twice < d) up = false;
      else if (mode === "HalfUp") up = true;
      else if (mode === "HalfDown") up = false;
      else up = q % 2n !== 0n; // HalfEven: a tie goes to the even neighbour.
    }
  }
  const mag = up ? q + 1n : q;
  return negative ? -mag : mag;
}

/**
 * `d.round(scale, mode)`: the value at exactly `scale` fraction digits. A
 * scale above `d`'s pads with zeros (no rounding); a smaller one rounds by
 * `mode`. A negative scale is a fault.
 */
export function __bynkDecimalRound(
  d: __BynkDecimalValue,
  scale: number,
  mode: __BynkRoundingMode,
): __BynkDecimalValue {
  checkScale(scale, "round");
  if (scale >= d.scale) return __bynkDecimal(lift(d, scale), scale);
  const negative = d.coefficient < 0n;
  const mag = negative ? -d.coefficient : d.coefficient;
  return __bynkDecimal(divRound(mag, pow10(d.scale - scale), negative, mode), scale);
}

/**
 * `a.div(b, scale, mode)`: the quotient at exactly `scale` fraction digits,
 * rounded by `mode`. A zero divisor is a fault, as `Int` division is.
 */
export function __bynkDecimalDiv(
  a: __BynkDecimalValue,
  b: __BynkDecimalValue,
  scale: number,
  mode: __BynkRoundingMode,
): __BynkDecimalValue {
  if (b.coefficient === 0n) throw new Error("Decimal division by zero");
  checkScale(scale, "div");
  // a/b = (ca / 10^sa) / (cb / 10^sb); the result coefficient at `scale` is
  // round(ca * 10^(scale + sb) / (cb * 10^sa)).
  const negative = a.coefficient < 0n !== b.coefficient < 0n;
  const num = (a.coefficient < 0n ? -a.coefficient : a.coefficient) * pow10(scale + b.scale);
  const den = (b.coefficient < 0n ? -b.coefficient : b.coefficient) * pow10(a.scale);
  return __bynkDecimal(divRound(num, den, negative, mode), scale);
}

/** `Decimal.fromInt(n)`: exact, at scale 0. */
export function __bynkDecimalFromInt(n: number): __BynkDecimalValue {
  return __bynkDecimal(BigInt(n), 0);
}

/** `d.toInt()`: `Some` when `d` is a whole number within the safe-`Int` range. */
export function __bynkDecimalToInt(d: __BynkDecimalValue): Option<number> {
  const p = pow10(d.scale);
  if (d.coefficient % p !== 0n) return None;
  const whole = d.coefficient / p;
  if (whole > BigInt(Number.MAX_SAFE_INTEGER) || whole < BigInt(Number.MIN_SAFE_INTEGER)) {
    return None;
  }
  return Some(Number(whole));
}

/** `d.toFloat()`: the nearest `Float` (lossy, by design). */
export function __bynkDecimalToFloat(d: __BynkDecimalValue): number {
  return Number(__bynkDecimalToString(d));
}

/** The canonical text, `scale` fraction digits: `1.20`, `-0.05`, `7`. */
export function __bynkDecimalToString(d: __BynkDecimalValue): string {
  const negative = d.coefficient < 0n;
  let digits = (negative ? -d.coefficient : d.coefficient).toString();
  if (d.scale === 0) return (negative ? "-" : "") + digits;
  if (digits.length <= d.scale) digits = "0".repeat(d.scale + 1 - digits.length) + digits;
  const cut = digits.length - d.scale;
  return (negative ? "-" : "") + digits.slice(0, cut) + "." + digits.slice(cut);
}

const CANONICAL = /^(-?)([0-9]+)(?:\.([0-9]+))?$/;

/**
 * `Decimal.parse(s)`: `Some` for plain decimal text (`"1.20"`, `"-0.5"`, `"7"`),
 * `None` for anything else - an exponent, a leading `+`, a bare `.5` or `5.`,
 * whitespace, or text over 1000 characters. The scale is the number of
 * fraction digits written.
 */
export function __bynkDecimalParse(s: string): Option<__BynkDecimalValue> {
  if (s.length > MAX_TEXT) return None;
  const m = CANONICAL.exec(s);
  if (m === null) return None;
  const frac = m[3] ?? "";
  const coefficient = BigInt(m[2] + frac);
  return Some(__bynkDecimal(m[1] === "-" ? -coefficient : coefficient, frac.length));
}

const CANONICAL_WIRE = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?$/;

/**
 * The wire decoder: `__bynkDecimalParse` with leading zeros refused too
 * (`"007"`, `"-01.5"`), so the wire form is canonical. `"0.5"` and `"-0"` pass.
 */
export function __bynkDecimalParseWire(s: string): Option<__BynkDecimalValue> {
  return CANONICAL_WIRE.test(s) ? __bynkDecimalParse(s) : None;
}

/**
 * The value-canonical text of `d` - trailing fraction zeros dropped - so two
 * numerically equal decimals encode alike (`Map` keys, `Set` elements).
 */
export function __bynkDecimalKey(d: __BynkDecimalValue): string {
  let c = d.coefficient;
  let s = d.scale;
  while (s > 0 && c % TEN === 0n) {
    c /= TEN;
    s -= 1;
  }
  return __bynkDecimalToString(__bynkDecimal(c, s));
}

/**
 * A failure-message rendering of any value that may hold a decimal: a decimal
 * prints as its text with a `d` suffix (`1.10d`), wherever it sits. A test
 * runner's `expect` uses it for a comparison whose operands mention `Decimal`,
 * since `JSON.stringify` throws on the `bigint` coefficient.
 */
export function __bynkDecimalShow(v: unknown): string {
  if (__bynkIsDecimal(v)) return __bynkDecimalToString(v) + "d";
  try {
    return (
      JSON.stringify(v, (_k, x) => (__bynkIsDecimal(x) ? __bynkDecimalToString(x) + "d" : x)) ??
      String(v)
    );
  } catch {
    return String(v);
  }
}
