// #2098: hand-mirrored TS types for `bynk-lsp`'s `bynk/explain` wire shape
// (`bynk-lsp/src/explain_request.rs`). Maintained by hand alongside the Rust
// side, the same posture `docTypes.ts` takes for the documentation model.

/** A curated diagnostic explanation, from the installed toolchain's own table. */
export interface Explanation {
  /** The diagnostic code, e.g. `bynk.resolve.unknown_type`. */
  title: string;
  /** The blurb, then the example as a fenced `bynk` block. No link, no raw HTML. */
  markdown: string;
  /** The hosted-Book page for the code (the diagnostic's `codeDescription`). */
  href: string;
}

/** What the explanation webview reads from `window.__BYNK_EXPLAIN__`. */
export interface ExplainPayload {
  explanation: Explanation;
}
