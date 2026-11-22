---
level: patch
changelog: Internal hardening in the code generator; the last print-then-splice round trips in the emitter become syntax-tree nodes
probes: emit_print_calls, ts_writes, verbatim_sites, ts_text_idents
---
refactor(emit): the last print round trips become nodes (1A.39a)

Closes #3856. Part of #3930. Part of #3676.

`emit_print_calls` reads 1, the driver's test-harness entry `extend_printed_at`, which stays until 1B.2 moves `tests_emit` into `bynk-emit`. The lazy field spec, the ICU `select` arm, the witness decoder, the store handles, the agent state zero values, the single-file header and the suite declarations are nodes.

**Probes.** `emit_print_calls` 27 to 1; `ts_text_idents` 12 to 8 (the store handles and `agent_key_assign`); `verbatim_sites` 12 to 9; `ts_writes` 680 to 649. No other gated probe moved.
