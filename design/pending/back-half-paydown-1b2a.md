---
level: patch
changelog: Internal restructuring: test-harness generation moves from the driver into `bynk-emit::testgen`; no emitted output changes
probes: ts_raw_sites_emit, ts_raw_sites_testgen, emit_diagnostics, testgen_print_calls
raises: ts_raw_sites_emit — reclassified from ts_raw_sites_testgen: the production commons barrel (sum unchanged)
---
refactor: test-harness generation moves into `bynk-emit::testgen` (1B.2a)

Part of #4028. Part of #3676.

No output changes. The generation half of `bynk-driver/src/project/tests_emit{.rs,/}` and the 16 test-runtime fragments move, with `git mv`, to `bynk-emit/src/testgen{.rs,/}` and `testgen/test_runtime/`. The driver keeps orchestration, file I/O, process spawning and `mod remote_tests` (now `project/remote_tests.rs`), and calls `process_tests`, `process_integration_tests`, `discovery_manifest` and `emit_test_main`; the one predicate that stays there, `project_mentions_encode_plain`, is passed in as a `bool`. `RemoteTests` moves to `emit_model`, and `SMOKE_SUITE_SUFFIX`, `TEST_SESSION_KEY` and `EVAL_CASE` to `testgen`, each re-exported under its old driver path.

The commons barrel is production output, so it moves to `emitter/commons_barrel.rs`: `ts_raw_sites_emit` goes from 4 to 6 and `ts_raw_sites_testgen` from 58 to 56 (the sum is unchanged). `emit_print_calls`, `import_prediction_fns`, `emit_silent_fallbacks` and `ast_importers` skip `testgen`, as `ts_raw_sites_emit` does, and read what they read before. `testgen_print_calls` is new: a gated growth probe over the harness, starting at 43. `emit_diagnostics`' naive halves move because the moved code carries dotted non-code literals. The 27 wildcard arms `bynk-emit` denies are exhaustive. `system_support.rs` is split as it moves, so that none of the files arrives over 2,000 production lines.

## ADR: where-test-generation-lives
title: Test-harness generation is `bynk-emit::testgen`, and the driver only orchestrates it
summary: The test modules, entry and discovery manifest are generated in `bynk-emit`; the driver keeps orchestration, I/O and spawning; `bynk-emit` has no dependency on `bynk-driver`, including for its tests

**Context.** Crate-hygiene D1 (#2982) moved the `project` module, and with it the test-harness generator `tests_emit`, from `bynk-emit` to `bynk-driver`. The back-half review (17 November 2026, §3) found that about 60% of the driver's production code then generated TypeScript, and that `bynk-emit` took a dev-dependency on `bynk-driver` so that its tests could drive the pipeline. The back-half paydown track settled where the generator belongs (§9 Q3) and asked for this record with 1B.2 (Q7). A separate crate would add a publish-list entry, a version and a dependency edge for code that only the driver calls.

**Decision.** Test-harness generation is a module, `bynk_emit::testgen`, beside `emitter`. It builds each unit's and integration suite's test module, the test entry (`tests/main.ts`), the discovery manifest and the embedded test-runtime fragments, and it returns staged files and `TsProgram`s. It reads no file, spawns no process and holds no thread-local; the `fs_below_driver`, `emit_thread_locals` and `hoist_sinks` gates cover it. `bynk-driver` keeps orchestration (`run_checks`, `build_output`), file I/O, process spawning and running tests, and calls `testgen`. Production output, including the commons barrel, never lives in `testgen`. The probes read production emission and test-harness generation apart, by `is_testgen_path`: `ts_raw_sites_emit` and `ts_raw_sites_testgen`, `emit_print_calls` and `testgen_print_calls`; and `ast_importers`, `import_prediction_fns` and `emit_silent_fallbacks` count production only. A `bynk-emit` test that needs the whole pipeline lives in `bynk-tests`; `bynk-emit` has no dependency on `bynk-driver` of any kind. `testgen` becomes a crate only if compile times measurably need it.

**Consequences.** The dependency graph has no cycle, including dev-dependencies (the §8 row). The driver's production code falls by about 12,500 lines and `bynk-emit`'s rises by the same, so the test runtime (1B.3), the shared node builders (1B.4) and the conversion of the harness to nodes (1B.5) each work in one crate. `bynk-emit`'s public API gains the entries the driver calls. The test modules keep predicted imports until 1B.5 converts their bodies to nodes. Their text stays outside the identifier audit until then. The probes' production readings are unaffected by harness code, and the harness's text escapes stay measured, and ratcheted, in their own rows.
