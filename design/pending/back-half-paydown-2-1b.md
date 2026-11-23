---
level: patch
changelog: Internal restructuring: `emit.rs` splits further by declaration kind (handler names, messages and capabilities move out); no output changes
---
refactor: `emit.rs` splits by declaration kind, part two: handlers, messages and capabilities (2.1b)

Part of #4086 (slice 2.1; the last part, 2.1g, closes it). Part of #3676.

No output changes. The handler method names and labels, with the handler-body items that services, agents and free functions share, move into `emitter/emit/handlers.rs`; `messages` bundles and ICU placeholders, with their tests, into `emit/messages.rs`; and capabilities, derived capabilities and providers, with the provider ordering, into `emit/capability.rs`. Function bodies are moved unchanged. `emit.rs` re-exports all three (`pub use` for `handlers`, whose public items `workers` and `workers_entry` use, and `pub(crate) use` for the other two), so every `emitter::emit::X` and `emitter::X` path resolves as before; each child imports explicitly. Ten items become `pub(super)` because the parent or a sibling calls them: eight handler-body items (`events_decl_item`, `events_flush_item`, `events_release_body`, `iife_const_item`, `join_plus`, `result_closure_item`, `return_result_item`, `return_undefined_unless`) and two capability items (`append_deps_field`, `seam_identity_ts_type`). `AST_IMPORTER_ORIGINS` gains the three children, so `ast_importers` is unchanged. `emit.rs` keeps its module-line ceiling (2.0 Decision D: a shrunken file may keep its old ceiling; the Phase 2 tidy PR lowers them). No gated probe moves, and no function-length ceiling is touched.
