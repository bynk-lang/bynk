---
level: patch
changelog: Internal hardening in the code generator; the last text identifiers and text-built types in the emitter become syntax-tree nodes, and a corpus audit now checks every identifier in the built trees
probes: ts_text_idents, ts_writes
---
refactor(emit): the last text identifiers and text-built types become nodes (1A.39b)

Closes #3930. Part of #3676.

`ts_text_idents` reads 0 with its P0.2 definition unchanged, meeting the §8 Phase 1 exit row. Generated names have one spelling each (`commons_alias`, `entry_fields_const`, `__d_{field}`, the harness's `env_binding`); the bundle factory's `Object.assign`, the codec depth arguments and guard, and the Decimal violation text are nodes; the actor-sum and call-sum unions are `TsType` nodes (the member separator in those types goes from `,` to `;`, a type-equivalent change in 19 corpus files). `expr_contains_await` is structural, and `TsArrowBody::Flat` documents and asserts its trimming.

The new identifier audit (`bynk-tests/tests/integration/ident_audit.rs`, `bynk_ts::text_idents`) found far more text in `Ident` than the line probe can see. What remains is accounted by structural class, each with a reason and the owner 1A.40, and the audit fails on a find outside every class and on a class that no longer occurs.

**Probes.** `ts_text_idents` 8 to 0; `ts_writes` 649 to 633. `emit_print_calls` stays 1 and `verbatim_sites` stays 9.
