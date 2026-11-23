---
level: patch
changelog: Internal restructuring: the `?`/control-flow and operator lowering move out of `lower.rs` into their own modules
---
refactor: split lower.rs, part three (2.2c)

Part of #4087. Part of #3676.

A pure move: the `?` and value-position control flow family (`lower/control.rs`:
`?`, the value-position `if`, lambdas, block expressions, the `match` IIFE) and
the operator family (`lower/operator.rs`: binary, short-circuit and equality
operators) leave `lower.rs`, which re-exports them, so no caller changes.
Function bodies are byte-identical; output is unchanged.

No gated probe moves: `ast_importers` is unchanged because the two new files are
entered in `AST_IMPORTER_ORIGINS` (origin `emitter/lower.rs`), so no `probes:`
line. `lower.rs` now reads under 2,000 production lines, so its entry leaves
`xtask/module-line-ceilings.txt` (the two-way rule requires it) and
`GRANDFATHERED` in `xtask/src/module_lines.rs`.
