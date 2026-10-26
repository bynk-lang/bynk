---
level: minor
probes: ts_writes
changelog: The language server answers `textDocument/inlineValue`, so a paused Bynk debug session shows each in-scope `let` and parameter beside its line, in Bynk's rendering
---

## ADR: inline-values-from-the-language-server
title: Inline values come from the language server, named by the emitter's own mapping
summary: textDocument/inlineValue returns variable lookups; the Bynk-to-emitted name rule is shared, not re-derived

**Context.** While paused in a handler, values appear in the Variables pane but
not beside the code, and VS Code's fallback guesses from identifiers. A provider
must tell the debugger which variable to look up, and the debugger sees the
emitted TypeScript names, not Bynk's. The language server does not depend on the
emitter.

**Decision.** `bynk-lsp` serves `textDocument/inlineValue`, returning an
`InlineValueVariableLookup` for each local declared in the requested range and
in scope at the stopped location, so the debug adapter resolves the value (and
the existing value rendering applies) and any LSP client benefits. The name is
never guessed: `ts_ident`, the emitter's binding-name rule, moves to
`bynk_check::emitted_names` and the emitter re-uses it, and
`emitted_local_name` mirrors the emitter's `bind_local_name` choice. A `let`
that re-binds a name already in scope is emitted as a throwaway `__r<N>` from a
counter spanning the whole body, which the source cannot predict; it is omitted
rather than looked up under a wrong name. A compile-and-compare test pins the
mapping to the emitted TypeScript.

**Consequences.** No language, checker or emitter behaviour changes, and no new
`vscode-bynk` code (the client enables the feature). Re-`let` bindings show no
inline value (they stay in the Variables pane). If the emitter's local naming
changes, `emitted_names` and its test change with it.
