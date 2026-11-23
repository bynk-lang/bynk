---
level: patch
changelog: Internal restructuring: the boundary-codec generator's shared vocabulary, field codecs and value codecs move into their own modules
---
refactor: split serialisation.rs, part one (2.3a)

Part of #4088. Part of #3676.

The first of two parts. A pure move: the shared codec vocabulary
(`serialisation/support.rs`), the field codecs and their wire forms and defaults
(`serialisation/field.rs`, with `default_lowering_tests`) and the value codecs at a
boundary (`serialisation/expr.rs`, with the serialiser corpus) leave
`serialisation.rs`, which re-exports the items named outside the family, so no
caller changes. `serialiser_corpus.txt` moves beside `expr.rs`. Function bodies
are byte-identical; output is unchanged.

No gated probe moves: `ast_importers` reads 13 because `serialisation.rs` is an
`AST_IMPORTER_EXCEPTIONS` entry and its children are entered in
`AST_IMPORTER_ORIGINS`, which the exclusion now follows (a file split out of an
excluded origin is excluded; a new file in the directory is still counted), so no
`probes:` line.
