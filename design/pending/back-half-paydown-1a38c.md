---
level: patch
changelog: The state-migration step functions the compiler writes into an agent's module are built as trees, not text, so each statement of a step prints on its own line and its `catch` is braced; their tokens, and the output of every other construct, are unchanged
probes: ts_writes, ts_raw_sites_emit, emit_print_calls
---
refactor(emit): the migration step function and the codec round trips become nodes (1A.38c)

Closes #3745. Part of #3676.

The last non-survivor `TsStmt::raw(` site, the `migrate from N` step function in `emit.rs`, is built as nodes by `migration_step_decl`, with the old record's fields, the carried state, the assignments and the encode back to the wire as statements. `deserialise_expr` takes its input as a `TsExpr`. The codec builders take their value type as a `TsType`, the `Set` and `Map` constructors are `TypeApp` callees, and a `Query` annotation is a new `TsType::Paren`, so the printed-then-wrapped type names are gone.

`ts_raw_sites_emit` falls by 1 (5 to 4: the four fixed authored snippets). `ts_writes` falls from 715 to 681 and `emit_print_calls` from 44 to 28.
