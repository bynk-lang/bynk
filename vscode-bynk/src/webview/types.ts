// #846: hand-mirrored TS types for `bynk-lsp`'s `bynk/sequenceModel` wire
// shape (`bynk-lsp/src/sequence_request.rs`). No codegen exists in this repo
// for LSP payload types (no `schemars`/similar), so this is maintained by
// hand alongside the Rust side — the same posture every other custom
// LSP-adjacent shape in this extension takes.

export type ParticipantKind = "Entry" | "Capability" | "Context" | "Agent" | "Actor";
/** `StateWrite` (#2194): an agent handler's write to its own state (`status :=
 *  Placed`, `items.put(id, …)`). `from === to ===` the entry; rendered as a
 *  note over the entry lifeline, not an arrow. */
export type MessageKind = "Call" | "Return" | "Send" | "StateWrite";
export type AltKind = "If" | "Match" | "Collapsed";

export interface Position {
  line: number;
  character: number;
}

export interface Range {
  start: Position;
  end: Position;
}

export interface Participant {
  id: number;
  kind: ParticipantKind;
  name: string;
  range: Range | null;
}

export interface Message {
  from: number;
  to: number;
  kind: MessageKind;
  label: string;
  range: Range;
  block: number | null;
}

export interface Branch {
  label: string;
  messageIds: number[];
  /** The value the handler yields on this branch (`Ok(view)`) — rendered as a
   *  note over the entry lifeline so a return-gating block (whose branches
   *  call no lifeline) has content instead of collapsing to an empty `alt`.
   *  `null` when the tail carries no distinguishable reply. */
  reply: string | null;
}

export interface AltBlock {
  id: number;
  kind: AltKind;
  branches: Branch[];
  range: Range;
  parent: number | null;
  parentBranch: number | null;
}

export interface SequenceModel {
  participants: Participant[];
  messages: Message[];
  blocks: AltBlock[];
}

/** The payload the extension host embeds into the webview — the model plus
 *  the document URI every range in it is relative to (the model itself
 *  carries no URI; the request that produced it already knows one file). */
export interface SequencePayload {
  model: SequenceModel;
  uri: string;
}

/** A click-to-code message the webview posts back to the extension host. */
export interface RevealMessage {
  type: "reveal";
  uri: string;
  range: Range;
}
