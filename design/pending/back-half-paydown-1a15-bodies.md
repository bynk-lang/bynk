---
level: patch
probes: ts_writes, ts_raw_sites_emit, stmt_text_lines
changelog: The code generator hands function and method bodies to the TypeScript printer as statements, and the printer records their source-map checkpoints, with output unchanged
---
refactor(emit): function and method bodies reach the tree as statements, and the printer owns their source maps (1A.15, first half)

First of the two PRs Decision F of #3716 allows. The statement emitter's entry points gain siblings that return `Vec<TsStmt>` (`function_body_stmts`, `tco::loop_body_stmts`, over `Body::into_stmts`): an `Item::Node` passes through, an `Item::Mark` puts its span on the next statement, and an `Item::Line` that remains becomes one padded `raw` statement carrying that span, at that one conversion site. An `Item::Indent` that holds a node (a loop body, the contract-guard closure) prints the node at its depth into one `raw` statement carrying its checkpoints as a `nested_map`, because a flat list of statements cannot nest one step deeper.

Callers converted: `emit_method`, `emit_free_fn` (plain, contract-guarded and self-tail-call loop bodies, with the `requires` and `ensures` guards built as body items), a provider's op methods and a WebSocket Durable Object method. Each puts the vector in its `TsDecl::Function`, `TsObjectEntry::Method` or `TsClassMethod` and loses its `body_text`, per-body `SourceMapBuilder`, `shift_checkpoints` and blob-offset search (`RawBody::flat` goes). The provider's class carrier stays one raw text blob and still merges its methods' map through `nested_map`; `print_class_method_and_merge` now records the body statements itself.

`bynk_ts` records every statement's span at any depth, as it prints: `print_with_map`, `print_stmt_and_merge`, `print_class_method_and_merge` and `print_object_entry_and_merge` share one implementation, and `print_stmt_with_spans` is that implementation over a throwaway map. The `SPAN_SINK` thread-local added by 1A.11 is deleted: the printer threads its map through the statement renderers explicitly, and `bynk-ts` has no production `thread_local!` left. Statements inside an arrow body still record nothing (no map is threaded into an expression); the second half does that, which is the one intended source-map change.

Left for the second half: service, agent and WebSocket-host handler bodies (prologues, the events and claim-scope wrappers), the claim-scope wrapper as nodes, `without_source_map` and the unmapped IIFE and lambda bodies.

Gated probes: `ts_writes` (1451 to 1447) and `ts_raw_sites_emit` (41 to 37) fall; `stmt_text_lines` stays at 32 (the loop and contract-guard wrapper text goes through one `Item::line` constructor, which also replaces the claim wrapper's). Goldens, source maps and the run fixtures are unchanged.
