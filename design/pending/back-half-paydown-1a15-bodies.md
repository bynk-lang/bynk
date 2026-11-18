---
level: patch
probes: ts_writes, ts_raw_sites_emit, stmt_text_lines
changelog: The code generator hands function, method and handler bodies to the TypeScript printer as statements, and the printer records their source-map checkpoints, so IIFE and lambda bodies are now mapped; output differs only in indentation
---
refactor(emit): lowered bodies reach the tree as statements, and the printer owns their source maps (1A.15)

Closes #3716. The statement emitter's entry points gain `Vec<TsStmt>` siblings over `Body::into_stmts`: an `Item::Node` passes through, an `Item::Mark` puts its span on the next statement, and an `Item::Line` that remains becomes one padded `raw` statement carrying that span, at that one conversion site. An `Item::Indent` that holds a node prints it at depth into a `raw` carrying its checkpoints as a `nested_map`. Every caller in the inventory now puts the vector straight into its body: `emit_method`, `emit_free_fn` (plain, contract-guarded and self-tail-call), a provider's op methods, a WebSocket Durable Object method, service handlers and agent handlers. Each loses its `body_text`, per-body `SourceMapBuilder`, `shift_checkpoints` and blob-offset search (`RawBody` and `emit_class_method_and_merge_source_map` are deleted). The provider, service and agent carriers stay one raw blob merged through `nested_map`; the track's declarations row records converting them.

`bynk_ts` records every statement's span at any depth as it prints, through `print_with_map`, `print_stmt_and_merge`, `print_class_method_and_merge`, `print_object_entry_and_merge` and `print_stmt_with_spans`, and the map is threaded explicitly through the expression renderers, so statements inside a multi-line arrow body record spans and merge their `nested_map` too. The 1A.11 `SPAN_SINK` thread-local is deleted; `bynk-ts` has no production `thread_local!`.

The claim-scope wrapper is nodes: `let __r;`, a `try` over an async closure assigning the body's result, a `catch` that releases the claims and rethrows, then the confirmation and the return. The `__claims` initialiser stays one text statement (`CLAIM_SCOPE_DECLARATION`), because its type assertion carries a generic method signature (`note<C>(...)`) that `bynk_ts` type members cannot express. The printer's layout puts the body one `try` block and one closure deeper than the text wrapper did, so six golden files change in indentation only.

`without_source_map`, `Body::to_nodes_unmapped` and the `mapped` parameter of the `match` emitters are deleted. IIFE, `match`-IIFE and lambda bodies keep their marks, so they are mapped for the first time: the one intended source-map change, which ends the `bynk test --inspect` degradation inside them. Two new inspector-style tests step into a lambda body and into a value-position `if`; both fail before this change. A text fallback of such a body renders through `render_text`, which records nothing. A lambda reached through the text method-call dispatchers (for example a list `.map`) is still text and stays unmapped until those dispatchers become nodes.

The value-position `match` carry-over from 1A.14: `match_body` writes each arm tail through `tail_item`, and `lower_expr`'s `Match` arm keeps the node. The 1A.13 nit tests keep their byte pins and gain node-path assertions (no `Item::Line` rendered).

Left as text on purpose: the events-release closure lines (`const __result = await (async () => {` and `})();`) and the subscriber and schema prologue guards stay `Item::line` text, because the handler body sits at the closure's own indentation, and a nested arrow body would indent it one step deeper, which reindents 126 golden files. A follow-up row in the track covers that whitespace-only conversion.

Goldens: 84 files change, all whitespace only (`git diff -w` is empty): 6 for the claim wrapper, 78 for the value-position `match` layout. There are no `.map` goldens; maps are pinned by decode tests, which are unchanged apart from the new ones.

Gated probes: `ts_writes` (1451 to 1428), `ts_raw_sites_emit` (41 to 35) and `stmt_text_lines` (32 to 31) fall. `lowering_text_bridge` and `emit_thread_locals` do not move.
