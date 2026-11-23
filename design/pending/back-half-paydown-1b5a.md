---
level: patch
changelog: Internal restructuring: the test harness builds more of a generated test module as typed nodes (the commons barrel's helpers, the harness memo, `__bynkEnv`, fixture and asset tables, the snapshot table, event delivery helpers, `__bynkCases`, the PGlite teardown), and four fixed snippets become type-checked test-runtime modules; no emitted output changes
probes: ts_raw_sites_emit, ts_raw_sites_testgen, testgen_print_calls, ts_writes, env_protocol_literals
---
refactor: the harness's small statements and the commons barrel become nodes (1B.5a)

Part of #4054. Part of #3676.

Decision I: `__resetAgents`, the `__<Ctx>Deps` alias and `__makeSurface` in the commons barrel are an exported function, an exported type alias over an intersection and an exported function returning spreads, not `raw_using` text. Decision B: `CASE_TIMING_RUNTIME`, `TEST_FILTER_RUNTIME`, `STUB_OVERLAY_TS` and the fixed `__bynkAssetGet` are modules under `runtime/src/testing/` (`case_timing`, `test_filter`, `stub_overlay`, `asset_get`), type-checked with the rest of the runtime and spliced through `TestRuntimeModule`; `harness.ts` declares `__bynkAssetFiles`. The remaining small statements are nodes: the `__makeHarness` memo and the remote harness, `__bynkEnv`, the fixture constants, the asset and snapshot tables, the event delivery helpers, `__bynkCases`, the `Events.emit` loop and the PGlite teardown.

Output is byte-identical: no stored golden changes. The remote harness and the snapshot table have no golden, so unit tests pin their text. Two statements stay raw for a ruling on a layout class (the detached-report drain and print, and the snapshot report): their raw text has no trailing newline, so today's `tests/main.ts` runs the next statement onto the same line.
