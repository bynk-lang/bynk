// #2196: hand-mirrored TS types for `bynk-lsp`'s `bynk/agentStateModel` wire
// shape (`bynk-lsp/src/agent_state_request.rs`), maintained by hand like
// every other custom request's types in this extension.

import type { Range } from "./types";

export type StateTarget =
  | { kind: "State"; state: string }
  /** A computed write (`status := next(x)`, `status.update(f)`). */
  | { kind: "Unknown" }
  /** No write to the field on this path. */
  | { kind: "Unchanged" };

export interface StateEdge {
  handler: string;
  /** The states this path can start in. */
  from: string[];
  /** Conditions on the path that don't constrain the field, as source text. */
  guards: string[];
  to: StateTarget;
  /** The value the handler yields on this path; `null` for unit. */
  outcome: string | null;
  /** The write, or for an unchanged path the tail. */
  range: Range;
}

export interface StateMachine {
  field: string;
  fieldRange: Range;
  states: string[];
  initial: string | null;
  edges: StateEdge[];
  /** Reachable from `initial` (an `Unknown` edge reaches every state). */
  reachable: string[];
  /** Handlers that exceeded the extractor's path budget. */
  truncated: string[];
}

export interface AgentHandler {
  name: string;
  range: Range;
}

export interface AgentStateModel {
  agent: string;
  range: Range;
  handlers: AgentHandler[];
  machines: StateMachine[];
}

export interface AgentStatePayload {
  model: AgentStateModel;
  uri: string;
}
