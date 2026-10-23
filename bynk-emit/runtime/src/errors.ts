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
//
// `invariant` names the declaration that was violated, which is an `invariant` or a
// `transition` (`declaration` says which): an agent may declare one of each under a
// single name, and the message is the same for both, so the name alone does not say
// which fired. The generated check passes it (#2272); a caller that does not leaves
// it absent.
export interface InvariantViolation {
  readonly kind: "InvariantViolation";
  readonly agent: string;
  readonly invariant: string;
  readonly declaration?: "invariant" | "transition";
}

export function invariantViolation(
  agent: string,
  invariant: string,
  declaration?: "invariant" | "transition",
): Error {
  const e = new Error(`InvariantViolation: ${agent}.${invariant}`);
  (e as { invariantViolation?: InvariantViolation }).invariantViolation = {
    kind: "InvariantViolation",
    agent,
    invariant,
    ...(declaration === undefined ? {} : { declaration }),
  };
  return e;
}
