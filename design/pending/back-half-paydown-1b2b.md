---
level: patch
changelog: Internal restructuring: `bynk-emit` no longer depends on `bynk-driver`, not even for its tests, and the test-harness lowering entries move into `bynk-emit::testgen`; no emitted output changes
probes: emit_print_calls, testgen_print_calls, emit_diagnostics
raises: testgen_print_calls — moved from production emission (1B.2b), not new
---
refactor: `bynk-emit` stops depending on `bynk-driver` (1B.2b)

Part of #4028. Part of #3676.

No output changes. `bynk-emit`'s `[dev-dependencies]` goes, so the crate graph has no cycle, including for tests (the §8 row). The 52 `bynk-emit` tests that drove the driver's in-memory pipeline, and the 3 tests of the helpers themselves, move into `bynk-tests/tests/integration/` with every assertion kept; `testkit` keeps `emit_source`. The README's usage example, a doctest that needed the driver, runs there too (`emit_readme_example`), with a check that it still matches the README block. `extend_printed_at` and the test-harness lowering entries move from `emitter/lower.rs` and `emitter.rs` into `testgen/case_body.rs`, so `emit_print_calls` reads 0 and `testgen_print_calls` 44. `emit_diagnostics` falls (`bynk-emit` 4/8 to 1/5, true/naive) because the three `bynk.project.contracts_lock_*` codes sat in the moved contracts-lock tests; no diagnostic code was added or removed.
