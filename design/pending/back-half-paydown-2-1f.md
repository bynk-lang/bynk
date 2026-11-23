---
level: patch
changelog: Internal restructuring: `emit.rs` splits further by declaration kind (the agent state persistence and history driver move out); no output changes
---
refactor: `emit.rs` splits by declaration kind, part six: agent state and agent history driver (2.1f)

Part of #4086 (slice 2.1; the last part, 2.1g, closes it). Part of #3676.

No output changes. Two files come out of `emit.rs`, function bodies unchanged:

- `emitter/emit/agent_state.rs` holds `loadState` and `commitState` (`emit_agent_load_commit_state`), hydration, and the persistence guards and entries (`emit_agent_state_persistence`, `agent_state_checks_and_entries`, the `persist_*` builders).
- `emitter/emit/agent_history.rs` holds the history-property driver (`emit_agent_history_driver`) and the `history_*` builders it assembles.

Everything in both is private to `emit`, so `emit.rs` takes them with plain `use` globs. Both children import explicitly. Eleven items become `pub(super)` because the parent or a sibling uses them: `AgentLoadFns`, `AgentPersistFns`, `emit_agent_load_commit_state`, `emit_agent_state_persistence`, `emit_agent_history_driver`, `persist_global`, `persist_present`, `persist_recast`, `persist_record_ty`, `persist_json_input` and `wire_record_ty`. So do the twenty-two fields of `AgentLoadFns` (12) and `AgentPersistFns` (10). `emit_agent_load_commit_state`'s function-length ceiling is re-keyed to `emit/agent_state.rs` at its current value (550). `AST_IMPORTER_ORIGINS` gains both files in the `emitter/emit.rs` group, in path order, so `ast_importers` is unchanged. No gated probe moves, and the module-line ceilings and the track doc are not touched.
