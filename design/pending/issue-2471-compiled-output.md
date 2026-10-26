---
level: minor
changelog: "**Bynk: Show Compiled Output** in VS Code opens a file's generated TypeScript beside it, built from the unsaved buffers by the new `bynk compile --sources-stdin`, with the cursor highlighting matching lines in each editor through the emitter's source map"
---

## ADR: compiled-output-via-cli-overlay
title: Compiled output comes from the CLI, fed the unsaved buffers on stdin
summary: How the editor gets a file's generated TypeScript without the language server emitting
status: Accepted

**Context.** The proposal (#2471) wants a file's generated TypeScript shown
beside it and kept current as you work. It recommended a language-server
request that "reuses the analysed unit". The analysis round keeps no typed
program (only the binding index, hints and the like), and emission exists only in
`bynk_emit::project::compile_project`, which re-runs the checker. Runtime
emission is now per-module (`__bynk_runtime/<module>`, ADR 0615), so "the file's
output" also needs defining.

**Decision.** Three routes were considered.

- **Compile in the language server (rejected).** A `bynk/emittedOutput` request
  would compile the committed round's snapshots in memory. That works, and shows
  unsaved buffers, but `bynk-lsp` would take a `bynk-emit` dependency: this
  crosses the content-ownership boundary (#1086, P4.2 #1122) that keeps the
  server off the emitter, and it grew the release `bynkc-lsp` from 17.7 MB to
  23.5 MB (+33%, measured with `cargo build --release -p bynk-lsp`). A
  maintainer-level architecture change this feature does not justify.
- **Compile in WebAssembly in the extension (rejected).** `bynk-wasm` exists, but
  the extension would then carry its own compiler, which can drift from the
  toolchain the user has installed (the version skew `bynk doctor` exists to
  surface), and so show output the installed `bynk` would not emit.
- **The CLI, with an overlay on stdin (chosen).** `bynkc compile --sources-stdin`
  reads a JSON object `{ "<absolute path>": "<text>" }` from stdin and layers it
  over the sources read from disk as `CompileOptions::sources`, which
  `compile_project` already accepts. The extension runs it, through the new
  `bynk compile` driver command (which delegates to the resolved `bynkc`, as
  `bynk test` does, inheriting its resolution and skew gate), into a scratch
  directory, then reads each module and its source map. It uses the installed
  toolchain's own emitter, shows unsaved buffers, and leaves the language server
  and its boundary alone.

The overlay's rules: an entry for a source the build read replaces its text (the
overlay wins over disk); a `.bynk` path inside the project that it did not read
is added (an unsaved new file); any other path, outside the project or not
`.bynk`, is rejected and nothing is built, so a caller cannot believe a buffer
was compiled when it was not. Only `.bynk` sources are overlaid: a `bynk.toml`
edit is read from disk. `--sources-stdin` also makes the run a preview build,
reading and writing no `bynk.schema.lock`, so editing never changes the project
tree or fails on a lock reconciliation.

The file's output is every emitted `.ts` whose source map lists the file (two in
a Workers build; the client asks which). The per-module runtime has no source
file and is never part of it.

Sync is a highlight on cursor move, not locked scrolling. The map is line-level
(ADR 0103), so a source line maps to the run of generated lines of its nearest
enclosing statement, and a generated line to its statement's source lines.

**Consequences.** Each refresh is a full compile of the project in a fresh
process: 10 to 20 ms on the example projects here, more on a large one. The view
refreshes on a 500 ms debounce after an edit, on save, and on external changes,
only while it is open, and cancels a build a newer edit supersedes. Between an
edit and its rebuild the highlight is paused. Highlight granularity is the map's:
a statement, not a token. A project that stops compiling keeps the last good
output on screen. Needs `bynk` and `bynkc` installed (the extension's managed
toolchain provides both). Editor tooling only; the language, the emitter and the
runtime are unchanged. Hover, completion, semantic tokens and signature help are
unchanged, because no syntax changed.
