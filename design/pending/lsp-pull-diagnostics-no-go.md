---
level: patch
changelog: Evaluated LSP pull diagnostics (`textDocument/diagnostic`); `bynk-lsp` keeps push diagnostics
---

## ADR: lsp-keep-push-diagnostics
title: bynk-lsp keeps push diagnostics; pull diagnostics are not adopted
summary: Pull cannot reduce the analysis round that dominates large projects, so push stays

**Context.** LSP 3.17 lets a server answer `textDocument/diagnostic` and
`workspace/diagnostic` instead of (or beside) `textDocument/publishDiagnostics`.
`bynk-lsp` pushes: a debounced project round (`run_project_diagnostics`, ADR 0052)
analyses the whole project and `publish::publish_plan` turns the result into
publishes and clears. Issue #2477 asked whether pull would shorten the time to the
first diagnostic in the active file, and decided to measure before building.

**Measurement.** One project round is the content sweep plus
`bynk_ide::diagnose_project_with`, timed with `Instant` in a throwaway test inside
`bynk-lsp` (release build, empty overlay, one cold run then 14 warm runs). To
reproduce, put that test in the `content` module (it is private) and point it at
a project root.

On the examples:

| Example | Files | Cold | Warm median |
|---|---|---|---|
| `semver` (largest, 642 lines) | 4 | 5.7 ms | 3.1 ms |
| `markdown` | 6 | 2.3 ms | 1.7 ms |
| `orders` | 2 | 2.2 ms | 1.9 ms |

The examples are tiny, so a synthetic large project too: 50 copies of the
`semver` example (its `commons`, `context`, `service` and `suite` names suffixed
`_1`..`_50`, each context `uses` its own commons), giving 200 files and 32,100
lines with no diagnostics. That round took 332 ms cold and 256 ms warm median
(min 213, max 322): about 80x the 4-file example for 50x the lines, so slightly
superlinear.

The default `diagnostics_debounce_ms` is 300. On the examples analysis is about
1% of the time to the first diagnostic; on the 200-file project a round is about
as long as the debounce. Either way push and pull pay the same debounce and the
same whole-project round, and pull then adds a client round-trip
(`workspace/diagnostic/refresh`, then a `textDocument/diagnostic` request) that
push does not have. This is server-side only; client rendering is the same for
both.

**Decision.** Keep push. Do not implement pull diagnostics, and do not add the
client-capability detection that would gate them.

**Consequences.**
- Pull cannot reduce the round's cost: the same whole-project analysis runs
  either way, so per-document laziness saves no work, and stale rounds are
  already dropped by the generation and committed-round checks. Latency on large
  projects is a matter for incremental analysis, which #1523 (R3.15, the
  incremental scheduler decision) is deferred on; the large-project figure above
  is evidence for that issue's trigger.
- Pull would be a second delivery path to keep correct beside the publish plan's
  clears (ADR 0052) and the stale-while-revalidate behaviour (ADR 0235), and
  `tower-lsp` 0.20 only exposes the methods behind its `proposed` feature, which
  enables `lsp-types/proposed` crate-wide.
- Revisit if a supported client lacks push.
- Tooling delta: none (hover, completion, semantic tokens and signature help
  unchanged).
