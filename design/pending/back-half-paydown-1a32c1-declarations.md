---
level: patch
changelog: Eight more pieces of generated TypeScript are built as declaration and statement nodes rather than text, so a lazily read agent's `loadState` branch, a WebSocket agent's frame-decode `try`/`catch` and its close handler's `void` statements are laid out one statement per line (a ruled layout change in 46 generated files), and the lazy-read branch's last call now ends in a `;` (ruled, 38 files, semantically identical TypeScript); source maps are unchanged apart from the extra lines; what the emitted code does is otherwise unchanged
probes: ts_writes, ts_raw_sites_emit, verbatim_sites
---
refactor(emit): the remaining non-carrier raw sites, first half, lower to nodes (1A.32c-1)

Part of #3733. Part of #3676. #3733 closes with 1A.32c-2.

**What changed.** The refined-type guards, the messages table, the context deps interface, the lazy-read `if`/`else` in `loadState`, the migration dispatcher and the WebSocket frame decode, JSON parse and close prologue are nodes. `bynk_ts` gains `TsUnaryOp::Void` and a `doc` on `TsObjectEntry::Prop` (built through `prop` and `with_doc`), plus `debug_assert!` guards for a rest-and-optional parameter and a definite field whose type admits `undefined`; `print_object_entry_and_merge` is deleted. The WebSocket deps object's `...__givenDeps`, `...__eventsDeps` and `...{ env }` spreads are `Spread` entries.

**Rulings.**
1. Goldens: 46 `.ts` files blessed, layout only. 38 also gain a `;` after the lazy-read `else` branch's call.
2. Source maps: `carrier_source_map`'s WebSocket snapshot gains ten entries of the dispatch method's span, the extra generated lines of the ruled layout; no mapping moved. A corpus comparison found no file with identical TypeScript and different mappings.

**Moved on.** 5873, 6148 and 6186 (state hydrate, rehydrate, encode) to 1A.32c-2; 6396 (migration step function) to 1A.38.

**Probes.** `ts_raw_sites_emit` 31 to 23, `verbatim_sites` 14 to 13 (the WebSocket deps object is a node, not a tagged text expression), `ts_writes` falls.
