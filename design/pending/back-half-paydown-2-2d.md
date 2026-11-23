---
level: patch
changelog: Internal restructuring: the literal, record and identifier lowering move out of `lower.rs` into their own modules
---
refactor: split lower.rs, part four (2.2d)

Part of #4087. Part of #3676.

A pure move: the literals and records family (`lower/literal.rs`: scalar and
interpolated-string literals, record construction and spread) and the
identifiers and field-access family (`lower/ident.rs`: identifier lowering,
field access, map-key decoding, with its tests) leave `lower.rs`, which
re-exports them, so no caller changes. Function bodies are byte-identical;
output is unchanged.

No gated probe moves: `ast_importers` is unchanged because the two new files are
entered in `AST_IMPORTER_ORIGINS` (origin `emitter/lower.rs`), so no `probes:`
line. `lower.rs` was already under 2,000 production lines after 2.2c, so no
ceiling is touched.
