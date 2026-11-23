---
level: patch
changelog: Internal restructuring: the test harness builds each case, property, history and contract-attack runner of a generated test module as typed nodes instead of text, so a property runner's statements now print at the function's own indent and a test module's source map points a nested statement at its own source line; no behaviour changes
probes: ts_raw_sites_testgen, testgen_print_calls, ts_writes, ts_any
---
refactor: the case runners become nodes (1B.5b)

Part of #4054. Part of #3676.

The five runner kinds (unit case, integration/system case, value property, history property, contract attack) are built as statements and wrapped in one `async function` node, in place of a printed string wrapped in a raw statement. The lowering entries return statements: `lower_test_case_stmts` and `lower_block_to_async_stmts` for the unit and property runners, and `LoweredCaseBody` (statements, with the tail's source-map mark) for the integration runner. `emit_test_scope_setup`, `suite_fn_decls`, `suite_late_codecs` and the destructure helpers return statements. The `--settle` rewrite works on statements. `async_test_runner_fn`, `emit_statement`, `lower_test_case_body` and the `SuiteDecls` text are deleted.

Goldens: five ruled classes, all token-identical (the stored goldens compare equal with whitespace removed). Class A, runner indent: 19 test-module goldens. Class B, statements on one line print one per line: 5. Class C, a whitespace-only line prints empty: 1. Test-module source maps: the "nested statement maps its own line" class; only test-module `.map` files change.
