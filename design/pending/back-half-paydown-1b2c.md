---
level: patch
changelog: Internal restructuring: the test harness and the case lowering share one spelling for each generated name, and `bynk_ts` gains a `bigint` literal node; no emitted output changes
probes: ts_writes
---
refactor: one spelling per generated harness name, and `TsLit::BigInt` (1B.2c)

Part of #3676. Closes #4028.

No output changes. Each generated name the test harness builds with `format!` (the `__sysdrive_*` drivers, `worker_<ns>`, the route keys, the sibling and parent import specifiers, `contract <fn>`, `test_<i>`, `__seq_<n>`, `__bynkTest_<cap>`, `body: <param>, `, `__prop_<case>`) has one helper. The names the emitter's case lowering also spells are in `emitter/harness_names.rs`, so the caller and the definer of a `__sysdrive_*` driver cannot drift; the harness's own are in `testgen/names.rs`. `bynk_ts` gains `TsLit::BigInt` and `TsExpr::bigint`, which replace the `{n}n` text of the property generators, the Decimal coefficient and the predicate printer. `ts_writes` falls from 631 to 576, which meets the Phase 1 exit target (≤ 600).
