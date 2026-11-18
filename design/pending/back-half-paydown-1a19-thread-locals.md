---
level: patch
probes: ts_raw_sites_emit, ts_raw_sites_testgen, emit_thread_locals
changelog: The code generator carries a unit's source and hidden-type spellings in its context instead of thread-locals, with no change to the generated output or its source maps
---
refactor(emit): remove the two thread-locals (1A.19)

`FAULT_SOURCE` and `type_renames::SPELLINGS` stood in for per-unit context lowering could not reach. The unit's source and identity path (`UnitSource`) and its hidden-type spellings are now fields of `EmitProjectCtx`, filled by the driver where it already built the rest of the context; every `ModuleCtx` is built with them (`ModuleCtx::for_unit`, or `UnitFacts::detached()` for a scaffold lowered outside a unit), and every renderer takes a `TyRender` (the interner plus the spellings) where it took `tys`. Output is byte-identical, including source maps.

New gated growth probe `emit_thread_locals` counts unconditional `thread_local!` in `bynk-emit` production and reads 0 (it read 2); a new one needs a `raises:` with a reason. `bynk-ts`'s `SPAN_SINK` is not counted and goes with 1A.15.

`ts_raw_sites` is retired and replaced by two gated growth probes, `ts_raw_sites_emit` (production emission, 41) and `ts_raw_sites_testgen` (test-harness generation, `project/tests_emit/**`, 58), the same counting logic divided by path. This is a reclassification, not a fall: 41 + 58 = 99, the reading `ts_raw_sites` had. Its row in `design/greenfield-status.md` is replaced by the two new rows by hand, because the stamp only rewrites rows of live probes. The track doc's §8 exit targets for `ts_raw_sites` and for the `"any"` fallbacks are amended as the maintainer decided (2026-11-18), with §5 rows 1A.32c (now required), 1A.38 and 1B.5.

The text-exit counters (`TEXT_EXITS`, `TEXT_STMTS`, `LINES_RENDERED`) are also compiled in under a new `text-exit-counters` feature of `bynk-emit`, which `bynk-tests` enables, so an assertion on them through `bynk_driver` is real; the one vacuous assertion (the `QueueResult` variant call, comparing 0 with 0 through the driver) moved to `bynk-tests`.
