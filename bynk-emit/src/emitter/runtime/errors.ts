// #3952: the numeric argument of a rule, of whichever base the refined type has
// (`bynk.validation`'s `Bound`). A `Decimal` is the runtime's `{ coefficient, scale }`
// and a `Duration` its milliseconds, as those types erase everywhere else.
export type ValidationBound =
  | { readonly tag: "IntBound"; readonly value: number }
  | { readonly tag: "FloatBound"; readonly value: number }
  | { readonly tag: "DecimalBound"; readonly value: { readonly coefficient: bigint; readonly scale: number } }
  | { readonly tag: "DurationBound"; readonly value: number };

// #3952: the predicate a refinement check enforces, with its argument
// (`bynk.validation`'s `ValidationRule`); `WellFormed` is a value that is not a
// valid member of its base type at all. A plain tagged object, as the compiler
// shapes every sum, so a `match` on `e.rule` reads it with no conversion.
export type ValidationRule =
  | { readonly tag: "NonNegative" }
  | { readonly tag: "Positive" }
  | { readonly tag: "InRange"; readonly lo: ValidationBound; readonly hi: ValidationBound }
  | { readonly tag: "AtLeast"; readonly bound: ValidationBound }
  | { readonly tag: "AtMost"; readonly bound: ValidationBound }
  | { readonly tag: "NonEmpty" }
  | { readonly tag: "MinLength"; readonly limit: number }
  | { readonly tag: "MaxLength"; readonly limit: number }
  | { readonly tag: "Length"; readonly limit: number }
  | { readonly tag: "Matches"; readonly pattern: string }
  | { readonly tag: "IsoDate" }
  | { readonly tag: "Cron" }
  | { readonly tag: "WellFormed" };

export interface ValidationError {
  readonly field: string;
  readonly message: string;
  readonly rule: ValidationRule;
  readonly value: unknown;
}

// v0.22b: the JSON-decode error (ADR 0047) — BoundaryError's information
// (the discriminating kind and the tracked field path) flattened into a
// uniform, Bynk-inspectable record. `kind` is "Malformed" for a JSON.parse
// failure, else the BoundaryError kind ("StructuralMismatch",
// "RefinementViolation").
export interface DecodeError {
  readonly kind: string;
  readonly path: string;
  readonly message: string;
}

// v0.80 (§14): an agent invariant violated at the commit boundary. A dedicated
// internal fault — distinct from BoundaryError (the cross-Worker call/refinement
// layer) — thrown inside the generated `commitState` *before* the proposed state
// is persisted, so the offending commit never lands. It rides the existing
// uncaught-fault channel: the caller observes a fault, not an outcome (ADR 0107).
export interface InvariantViolation {
  readonly kind: "InvariantViolation";
  readonly agent: string;
  readonly invariant: string;
  // #2272: which declaration fired, and where it is. An invariant and a transition
  // may share a name, so the name alone does not say which one it was.
  readonly declaration: "invariant" | "transition";
  readonly location: string;
}

export function invariantViolation(
  agent: string,
  invariant: string,
  declaration: "invariant" | "transition",
  location: string,
): Error {
  const e = new Error(`InvariantViolation: ${agent}.${invariant}`);
  (e as { invariantViolation?: InvariantViolation }).invariantViolation = {
    kind: "InvariantViolation",
    agent,
    invariant,
    declaration,
    location,
  };
  return e;
}

// #2620: a deliberate fault — `unreachable(msg)` / `todo(msg)`. Like
// `invariantViolation` it rides the uncaught-fault channel, so a handler that
// reaches one faults and the agent commit never happens (ADR 0109); the caller
// observes a fault, not an outcome. `kind` and `location` ride on the thrown
// error for the logged fault. Returns `never`, so it is an expression of any type.
export interface Fault {
  readonly kind: "Unreachable" | "Todo";
  readonly message: string;
  readonly location: string;
}

export function fault(kind: "Unreachable" | "Todo", message: string, location: string): never {
  const e = new Error(`${kind}: ${message} (${location})`);
  (e as { fault?: Fault }).fault = { kind, message, location };
  throw e;
}

// #1653: the `__` aliases emitted code imports.
export { fault as __fault, invariantViolation as __invariantViolation };
