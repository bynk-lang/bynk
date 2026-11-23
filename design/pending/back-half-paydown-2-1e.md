---
level: patch
changelog: Internal restructuring: `emit.rs` splits further by declaration kind (the agent store layout and the agent migration steps move out); no output changes
---
refactor: `emit.rs` splits by declaration kind, part five: agent store layout and agent migration (2.1e)

Part of #4086 (slice 2.1; the last part, 2.1g, closes it). Part of #3676.

No output changes. Two files come out of `emit.rs`, function bodies unchanged:

- `emitter/emit/agent_store.rs` holds the store classification (`classify_agent_store`, `AgentStoreLayout`), the lazy-read specs, and the key and wire codecs. `type_base_is_tests` and `held_frame_ty_tests` move with it. `emit.rs` re-exports it with `pub(crate) use agent_store::*`, because `agent_state_ir`, `agent_reads_key`, `agent_key_codec_name` and `AgentStoreLayout` are reached from elsewhere in the crate.
- `emitter/emit/agent_migrate.rs` holds the `migrate from N` step functions and the dispatcher that runs them. `migrate_dispatcher_tests` moves with it. Everything in it is private to `emit`, so `emit.rs` takes it with a plain `use agent_migrate::*`.

Both children import explicitly. Eighteen private items become `pub(super)` because the parent or a sibling uses them: `agent_key_assign`, `agent_key_codec`, `agent_wire_name`, `agent_wire_table`, `classify_agent_store`, `declaration_location`, `emit_lazy_field_spec`, `history_variant_tag`, `is_codecable`, `object_assign_key`, `step_ranges_expr`, `store_field_key_type`, `type_base_is_int` and `type_base_is_string` in the store file; `MigrateNames`, `migrate_dispatcher_decl`, `migrate_step_dispatch` and `migration_step_decl` in the migration file. So do the seven fields of `MigrateNames`. `classify_agent_store`'s function-length ceiling is re-keyed to `emit/agent_store.rs` at its current value (310). `AST_IMPORTER_ORIGINS` gains both files in the `emitter/emit.rs` group, in path order, so `ast_importers` is unchanged. No gated probe moves, and the module-line ceilings and the track doc are not touched.
