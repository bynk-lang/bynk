// #847: hand-mirrored TS types for `bynk-lsp`'s `bynk/documentationModel` wire
// shape (`bynk-lsp/src/documentation_request.rs`). Maintained by hand alongside
// the Rust side, the same posture `webview/types.ts` takes for the sequence
// model — no LSP-payload codegen exists in this repo.

export interface Position {
  line: number;
  character: number;
}

export interface Range {
  start: Position;
  end: Position;
}

/** #2462: where an intra-doc link goes, resolved by the server. The entry's
 *  Markdown spells link `N` as `[text](#bynk-doc-link-N)`, `N` indexing the
 *  `links` list beside it. */
export type DocLink =
  | { kind: "entry"; entry: number }
  | { kind: "source"; uri: string; range: Range }
  | { kind: "external"; href: string };

/** One declaration on the page. `markdown` is a fenced `bynk` signature plus,
 *  when `documented`, the declaration's doc-comment prose — rendered
 *  HTML-disabled by the webview. */
export interface DocEntry {
  name: string;
  kind: string;
  depth: number;
  markdown: string;
  links: DocLink[];
  documented: boolean;
  range: Range;
  /** #1885: the document `range` is in. Always sent: the page merges a
   *  unit's files, so it may differ from the payload's `uri`. */
  uri: string;
}

export interface DocModel {
  unitName: string;
  unitKind: string;
  unitDoc: string | null;
  /** #2462: as `DocEntry.links`, for `unitDoc`. */
  unitLinks: DocLink[];
  unitRange: Range;
  entries: DocEntry[];
}

/** The payload the extension host embeds into the webview — the model plus the
 *  document URI its ranges are relative to (except an entry's own `uri`). */
export interface DocPayload {
  model: DocModel;
  uri: string;
}
