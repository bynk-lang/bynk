---
level: patch
changelog: Internal restructuring: the pattern and `is` lowering move out of `lower.rs` into their own modules
---
refactor: split lower.rs, part one (2.2a)

Part of #4087. Part of #3676.

A pure move: the pattern family (`lower/pattern.rs`) and the `is`/narrowing
family (`lower/is.rs`) leave `lower.rs`, which keeps the core and re-exports
both, so no caller changes. Function bodies are byte-identical; output is
unchanged.

No gated probe moves: `ast_importers` reads 13 because the two new files are
entered in `AST_IMPORTER_ORIGINS` (origin `emitter/lower.rs`), so no `probes:`
line. `lower.rs` now reads 3,752 production lines; its ceiling in
`xtask/module-line-ceilings.txt` stays loose until the final tidy.
