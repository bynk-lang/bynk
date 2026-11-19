---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: The persisted-store operations (held `Map`, `Map`, `Set`, `Cache`, `Log`, `Cell`) are built as TypeScript nodes rather than text, so each operation's statement IIFE is laid out as an indented block; no change to what the emitted code does
---
refactor(emit): the persisted-store calls lower to nodes (1A.23)

Closes #3724. Part of #3676.

**What changed.** The six store arms and `route_indexed_filter` return `Option<Built>` and build nodes over a `Member(Ident(var), field)` receiver, in `lower_call/store.rs` (moved first, mechanically; Decision E). The store arms hand 1A.22's node `Query` builders a node source, so 1A.22's print at the arm boundary is gone. Statement IIFEs use `TsArrowBody::Multiline`. `bynk_ts` gains `TsExpr::NonNull` (postfix `x!`). Beyond Decision A, the store read thunks convert too: the held-`Map`, `Map` and `Log` bare-identifier thunks and the `.keys` and `.values` field thunks.

**Goldens, as ruled.** 80 `.ts` files (31 Workers handlers, 49 bundle modules), layout only, all blessed: the store IIFEs as multi-line blocks, the 1A.22 column-0 re-indents (8 files) and the newly-node query pipelines. The whitespace-stripped text of every file is identical once the 3 two-declarator `const __ka = …, __kb = …;` sites (1 in 1977, 2 in 353) are read as two statements. Ruled: keep `TsExpr::NonNull`.

**Probes.** `ts_writes` 1371 to 1333; `lowering_text_bridge` 62 to 60; `ast_importers` and `ts_any` do not move.

**Named leftovers.** The `.entries` thunk stays text (no destructuring-parameter node); the unsupported-operation comment placeholders stay text, marked `// 1A: comment placeholder`.
