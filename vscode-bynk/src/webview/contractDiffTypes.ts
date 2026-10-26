// #2467: hand-mirrored TS types for `bynk-lsp`'s `bynk/contractDiff` wire
// shape (`bynk-lsp/src/contract_diff_request.rs`), maintained by hand like
// `architectureTypes.ts`. Project-scoped, so every location carries its own
// `uri`.

export interface Position {
  line: number;
  character: number;
}

export interface Range {
  start: Position;
  end: Position;
}

export interface CdLoc {
  uri: string;
  range: Range;
}

/** `compatible`: a build accepts it. `breaking`: `bynk check` refuses it. */
export type CdClass = "compatible" | "breaking";

export interface CdChange {
  kind: "event" | "agent";
  unit: string;
  name: string;
  class: CdClass;
  /** The schema version (event) or state version (agent) the lock records. */
  lockedVersion: number;
  detail: string;
  loc: CdLoc | null;
}

/** A hash comparison, never a verdict: services have no compatibility
 *  classifier (ADR 0491), so there is no `breaking` here. */
export type CdServiceStatus = "changed" | "added" | "removed";

export interface CdService {
  environment: string;
  context: string;
  service: string;
  status: CdServiceStatus;
  live: string | null;
  current: string | null;
  loc: CdLoc | null;
}

export type CdLedgerState = "absent" | "unreadable" | "read";

export interface CdModel {
  lockPresent: boolean;
  ledger: CdLedgerState;
  changes: CdChange[];
  services: CdService[];
}

export interface CdPayload {
  model: CdModel;
  uri: string;
}

export interface CdRevealMessage {
  type: "reveal";
  uri: string;
  range: Range;
}
