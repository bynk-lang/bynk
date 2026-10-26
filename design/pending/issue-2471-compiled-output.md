---
level: minor
changelog: "**Bynk: Show Compiled Output** in VS Code opens a file's generated TypeScript beside it, highlighting the matching lines in each editor through the emitter's source map (`bynk/emittedOutput`)"
---

## ADR: emitted-output-request
title: Compiled output is served by `bynk/emittedOutput`, which compiles the committed round in memory
summary: How the editor gets a file's generated TypeScript, and what "the file's output" means

**Context.** The proposal (#2471) wants a file's generated TypeScript shown
beside it, always current, without a full build to disk. It recommended a
language-server request that "reuses the analysed unit". The analysis round
keeps no typed program (only the binding index, hints and the like), and
`bynk-lsp` and `bynk-ide` deliberately do not depend on `bynk-emit` (the
content-ownership track, #1086; P4.2, #1122). Emission lives only in
`bynk_emit::project::compile_project`, which re-runs the checker. Runtime
emission is now per-module (`__bynk_runtime/<module>`, ADR 0615), so "the file's
output" needs defining.

**Decision.**

- **A request, not the disk.** `bynk/emittedOutput { textDocument }` returns the
  file's emitted TypeScript and its decoded source map. Reading the last
  `bynk build` from disk would show stale output and never an unsaved edit.
- **Compile the committed round's snapshots in memory.** The server builds a
  `CompileOptions` from the round's snapshots and the project manifest and calls
  `compile_project`. This costs a checker run, so the client asks only while a
  view is open (on open, then on the shared views refresh), never per keystroke.
  `bynk-lsp` therefore gains a `bynk-emit` dependency, confined to
  `emitted_request.rs`; the Cargo comment recording the old exclusion says so.
- **The file's output is the modules whose map names it.** Every emitted `.ts`
  document whose source map lists the file as a source, in output-path order
  (one per unit; two in a Workers build). The per-module runtime has no source
  file and is never part of it. A client shows one, asking when there are several.
- **Decode the map server-side.** The emitter's maps are line-level (ADR 0103),
  one segment per generated line, so the answer is a per-line
  `[source id, source line]` table; the client needs no VLQ decoder and the
  decoding is tested in Rust.
- **Highlight on cursor move, not locked scrolling.** Both directions use that
  table: a source line maps to the run of generated lines of its nearest
  enclosing statement, and a generated line to its statement's source lines.

**Consequences.** Editor tooling only; the language, the emitter and the
runtime are unchanged. Highlight granularity is the map's: a statement, not a
token. A project that stops compiling keeps the last good output in the view.
Hover, completion, semantic tokens and signature help are unchanged, because
no syntax changed.
