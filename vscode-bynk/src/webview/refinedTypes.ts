// #2470: hand-mirrored TS types for `bynk-lsp`'s `bynk/refinedTypeModel` wire
// shape (`bynk-lsp/src/refined_request.rs`) and the messages the explorer panel
// and its host exchange. Maintained by hand alongside the Rust side, as
// `explainTypes.ts` is.

/** One predicate of a refinement. */
export interface RefinedClause {
  /** As written: `InRange(1, 100)`. */
  source: string;
  /** In words: `1 ≤ value ≤ 100`. */
  description: string;
  /** The emitter's own condition over `value`, as JavaScript. */
  check: string;
  /** The emitter's failure message, as `.of` reports it. */
  message: string;
}

export interface RefinedModel {
  name: string;
  /** How a candidate is read: `Int`, `Float` or `String`. */
  base: "Int" | "Float" | "String";
  opaque: boolean;
  declaration: string;
  /** In the order the runtime checks them (a `Matches` last). */
  clauses: RefinedClause[];
  /** Candidate input texts; a string example is its own text. */
  examples: string[];
  examplesNote: string | null;
  seed: number;
}

/** What the explorer webview reads from `window.__BYNK_REFINED__`. */
export interface RefinedPayload {
  model: RefinedModel;
}

/** The verdict on one candidate. */
export type CandidateVerdict =
  /** The text is not a value of the base type, so no clause was run. */
  | { kind: "unreadable"; message: string }
  | { kind: "valid" }
  /** Every failing clause, in check order. */
  | { kind: "invalid"; failures: Array<{ source: string; message: string }> }
  | { kind: "timeout" };

/** Webview → host. */
export interface CheckMessage {
  type: "check";
  text: string;
}

/** Host → webview. */
export interface VerdictMessage {
  type: "verdict";
  text: string;
  verdict: CandidateVerdict;
}
