---
level: patch
changelog: Internal restructuring: `emit.rs` starts to split by declaration kind (types and functions move out); no output changes
---
refactor: `emit.rs` splits by declaration kind, part one: types and functions (2.1a)

Part of #4086 (slice 2.1; the last part, 2.1g, closes it). Part of #3676.

No output changes. The type, refined, record and sum declarations (`emit_type` and its checks) move into `emitter/emit/types.rs`, and attached methods, free functions, constants and contract guards into `emitter/emit/functions.rs`, with their tests. Function bodies are moved unchanged. `emit.rs` stays the parent and re-exports both (`pub(crate) use`), so every `emitter::emit::X` path resolves as before; each child imports explicitly, with no `use super::*`. Three functions become `pub(super)` because the parent or `types.rs` calls them (`emit_attached_methods`, `emit_free_fn_in`, `emit_const_in`). `AST_IMPORTER_ORIGINS` gains the two children, so `ast_importers` is unchanged. `emit.rs`'s module-line ceiling falls from 10,717 to 9,295. No gated probe moves, and no function-length ceiling is touched (none of the moved functions has one).
