export interface ValidationError {
  readonly field: string;
  readonly message: string;
  readonly value: unknown;
}

// v0.22b: the JSON-decode error (ADR 0047) — BoundaryError's information
// (the discriminating kind and the tracked field path) flattened into a
// uniform, Bynk-inspectable record. `kind` is "Malformed" for a JSON.parse
// failure, else the BoundaryError kind ("StructuralMismatch",
// "RefinementViolation").
export interface JsonError {
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
