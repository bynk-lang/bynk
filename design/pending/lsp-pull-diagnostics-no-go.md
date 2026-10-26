---
level: patch
changelog: Evaluated LSP pull diagnostics (`textDocument/diagnostic`); `bynk-lsp` keeps push diagnostics
---

## ADR: lsp-keep-push-diagnostics
title: bynk-lsp keeps push diagnostics; pull diagnostics are not adopted
summary: Measured time-to-first-diagnostic shows no gain from `textDocument/diagnostic`, so push stays

**Context.** LSP 3.17 lets a server answer `textDocument/diagnostic` and
`workspace/diagnostic` instead of (or beside) `textDocument/publishDiagnostics`.
`bynk-lsp` pushes: a debounced project round (`run_project_diagnostics`, ADR 0052)
analyses the whole project and `publish::publish_plan` turns the result into
publishes and clears. Issue #2477 asked whether pull would shorten the time to the
first diagnostic in the active file, and decided to measure before building.

**Measurement.** One project round is the content sweep plus
`bynk_ide::diagnose_project_with`, timed with `Instant` in a throwaway test inside
`bynk-lsp` (release build, empty overlay, one cold run then 14 warm runs) on the
examples:

| Example | Files | Cold | Warm median |
|---|---|---|---|
| `semver` (largest, 642 lines) | 4 | 5.7 ms | 3.1 ms |
| `markdown` | 6 | 2.3 ms | 1.7 ms |
| `orders` | 2 | 2.2 ms | 1.9 ms |

The default `diagnostics_debounce_ms` is 300. The first diagnostic therefore
arrives about 300 ms after the last edit, of which analysis is roughly 1%. Push
and pull pay the same debounce and the same round; pull then adds a client
round-trip (`workspace/diagnostic/refresh`, then a `textDocument/diagnostic`
request) that push does not have. This is server-side only; client rendering
is the same for both. The harness was not committed: it needs the private
`content` module and is a few lines to recreate.

**Decision.** Keep push. Do not implement pull diagnostics, and do not add the
client-capability detection that would gate them.

**Consequences.**
- No latency gain is available: the cost is the debounce, which pull does not
  change, and analysis is already whole-project, so pull's per-document
  laziness saves no work.
- Pull would be a second delivery path to keep correct beside the publish plan's
  clears (ADR 0052) and the stale-while-revalidate behaviour (ADR 0235), and
  `tower-lsp` 0.20 only exposes the methods behind its `proposed` feature, which
  enables `lsp-types/proposed` crate-wide.
- Revisit if a supported client lacks push, or if a round's cost approaches the
  debounce on real projects.
- Tooling delta: none (hover, completion, semantic tokens and signature help
  unchanged).
