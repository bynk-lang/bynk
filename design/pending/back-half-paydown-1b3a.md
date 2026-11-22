---
level: patch
changelog: Internal restructuring: the test runtime and the client runtime are authored as type-checked runtime modules and generated into the files bynk-emit embeds; no emitted output changes
probes: ts_raw_sites_testgen, ts_writes
---
refactor: the test runtime is first-party runtime code (1B.3a)

Part of #4047. Part of #3676.

No output changes. The 16 test-runtime fragments become modules under `bynk-emit/runtime/src/testing/`, and `http_client_runtime.ts` becomes `runtime/src/client/http_client.ts`; `bundle.mjs` writes today's files from them, byte for byte, and `--check` guards them. `testgen/runtime_splice.rs` is the one way the harness reaches a fragment. `ts_raw_sites_testgen` falls from 56 to 39 (20 raw sites become one `splice`, with the snapshot table and the observation newline staying raw), and `ts_writes` from 576 to 575.

## ADR: the-test-runtime-is-first-party-runtime-code
title: The test runtime is first-party runtime code
summary: Test-runtime code is authored as type-checked modules under the runtime's `src/testing/`, spliced into generated test modules as generated text, and guarded so every module is checked

**Context.** The back-half review (17 November 2026, section 3, corrected by #3689) found the test runtime spliced into each generated test module as script fragments: 16 files under `testgen/test_runtime/`, 8 of them not type-checkable alone because they use names that other fragments or the generated module define. Each was checked by `tsc --strict` only inside a module that included it, so a fragment no fixture included was not checked, and four were reached only by system tests. The client runtime that `bynk client` inlines (`http_client_runtime.ts`) was outside the runtime's `tsconfig.json` and `bundle.mjs --check`. The track asked for this record with 1B.3 (section 9 Q7, section 10), which absorbed the retired P0.6.

**Decision.** Test-runtime code is authored as modules under `bynk-emit/runtime/src/testing/`, type-checked with the rest of the runtime by its `tsconfig.json`, whether or not any fixture uses it. A module's dependency on another is an import; a name a generated test module supplies is declared in a contract module, `testing/harness.ts`, which is never spliced or emitted. The text a generated test module splices is a build artifact of `bundle.mjs` (the module with its relative imports dropped and its line-initial exports removed), guarded by `--check`, and it reaches the emitter only through the generated `test_runtime_modules.rs` table and the harness's `TestRuntimeModule`. A test fails if any other file includes an artifact. The client runtime is authored under `runtime/src/client/` and generated the same way. Test doubles that generated code imports are runtime modules under `testing/` and no production runtime module defines one (1B.3b). The harness decides from the trees it builds and does not read or patch emitted TypeScript text (1B.3c).

**Consequences.** Every line of test runtime and client runtime is checked under `tsc --strict` on every runtime change, and the free names between fragments are checked imports. Generated test modules are unchanged: they still splice the runtime text, so each keeps its own clock, retry count, snapshot state and timer queue. They move to importing the modules only when their imports are derived from the tree (1B.5), which this layout makes a change to one helper. The artifacts must not be edited by hand; `--check` fails if they are. The dependency order between spliced fragments, once implicit in where each call sat, is a debug assertion in the module assemblers.
