---
level: patch
changelog: Internal restructuring: `emit.rs` splits further by declaration kind (the WebSocket dispatch moves out); no output changes
---
refactor: `emit.rs` splits by declaration kind, part four: the WebSocket dispatch (2.1d)

Part of #4086 (slice 2.1; the last part, 2.1g, closes it). Part of #3676.

No output changes. The Durable Object methods that host a `from websocket` service's `on open`, `on message` and `on close` handlers, and the `fetch` upgrade branch that reaches them, move into `emitter/emit/websocket.rs`, function bodies unchanged. `emit.rs` re-exports it with `pub(crate) use websocket::*` (its widest item is `pub(crate)`), so every `emitter::emit::X` and `emitter::X` path resolves as before; the child imports explicitly. Fourteen items become `pub(super)` because the parent or a sibling uses them: `WsOpenHost`, `WsDepsRebuild`, `GivenDeps`, `agent_env_const_stmt`, `emit_ws_dispatch_handlers`, `emit_ws_do_method`, `events_deps_const_stmt`, `given_deps_const_stmt`, `wake_and_inspect_branches`, `ws_close_do_method_name`, `ws_message_do_method_name`, `ws_open_do_method_name`, `ws_open_fetch_branch_stmt` and `ws_open_hosts_for`. So do the thirteen fields of `WsOpenHost` and `WsDepsRebuild` and the four methods `has_inbound`, `reaches_env`, `is_empty` and `stmts`. No function in this file is over the function-length ceiling, so `xtask/fn-length-ceilings.txt` is untouched. `AST_IMPORTER_ORIGINS` gains `websocket.rs` in the `emitter/emit.rs` group, in path order, so `ast_importers` is unchanged. No gated probe moves, and the module-line ceilings and the track doc are not touched.
