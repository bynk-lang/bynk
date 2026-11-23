---
level: patch
changelog: Internal restructuring: the boundary-codec generator's refined, record, sum, generic and factory codecs move into their own modules
---
refactor: split serialisation.rs, part two (2.3b)

Part of #3676. Closes #4088.

The second of two parts. A pure move: the refined-scalar codecs and inline
refinement checks (`serialisation/refined.rs`), named records and sums
(`serialisation/record_sum.rs`), the generic-instantiation helpers and lenient
`Option` decode (`serialisation/generics.rs`) and the codec factories for user
generic types (`serialisation/factory.rs`), each with its tests, leave
`serialisation.rs`, which re-exports the items named outside the family, so no
caller changes. Function bodies are byte-identical; output is unchanged.
`emit_generic_helpers_qualified` moves with its function-length ceiling (keyed to
`serialisation/generics.rs`, 673 down to its measured 669).

`serialisation.rs` falls under 2,000 lines, so its module-size ceiling and its
`GRANDFATHERED` entry go.

No gated probe moves: `ast_importers` still reads 13 because `serialisation.rs` is
an `AST_IMPORTER_EXCEPTIONS` entry and the four new children are entered in
`AST_IMPORTER_ORIGINS`, which the exclusion follows (the origin-table exclusion
introduced in 2.3a, per the coordinator's ruling), so no `probes:` line.
