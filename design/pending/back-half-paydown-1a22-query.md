---
level: patch
probes: ts_writes
changelog: The `Query` operations over a store (`filter`, `sortBy`, `distinctBy`, `groupBy`, the joins, the terminals) are built as TypeScript nodes rather than text, so a statement-bodied step is laid out as an indented block; no change to what the emitted code does
---
refactor(emit): the `Query` vocabulary lowers to nodes; the `Stream` generators stay text (1A.22)

Closes #3723. Part of #3676.

**What changed.** `lower_query_method` takes a `QuerySource` (an array-valued source, or a `Query` receiver thunk to invoke) and node arguments, and returns `Option<Built>`: a node when the source and every argument are nodes, today's text otherwise. Every operation is its own builder over the pieces 1A.18 left shared in `lower_list.rs` (`with_comparator_node`, `kernel_key_comparator`, `join_other_elem`, `key_fn_is_plain`, the `distinct_by_body`, `join_body` and `group_by_body` kernels now shared with the list arms, and `sort_plain_by`). `QueryShape` carries the receiver's and the join's element types as `TsType`, so the printer owns the bracketing of `T[]`. The three store arms pass a node source (`__state.<field>.values()`, and the held `Map`'s `flatMap` over `__resolveConnection<F>`) and node arguments; the held, plain-`Map` and `Log` element types in the agent store state are now `TsType`. The vocabulary moved first, mechanically, to `lower_call/query.rs`.

**Goldens, as ruled.** 7 `.ts` files, layout only, 14 sites: a statement-bodied thunk (`groupBy` x3, `distinctBy` x2, plain-key `sortBy`/`sortByDescending` x4, `min`/`max` x5) goes from one line to an indented block. The whitespace-stripped text of every file is identical once a plain sort's `const __ka = …, __kb = …;` is read as two `const` statements (`bynk_ts` has no multi-declarator `const`; the same split as 1A.18b, 4 sites in 2 files). No bracket was dropped or gained, and no `__bynkThen` placement, keyed helper, comparator binding or source map changed. Interim, as ruled: the three store arms still return text, so their blocks sit at column 0 inside the one-line text until 1A.23 converts them. Also ruled: the agent-store maps `held_maps`, `map_values` and `log_values` hold `TsType` rather than `String`.

**Probes.** `ts_writes` falls (1371 to 1357 against 1A.21); `lowering_text_bridge`, `ts_any` and `ast_importers` do not move.

**Departures and named leftovers.**
- `Stream.map`, `take` and `collect` stay text, marked `// 1A: generator and for-await have no bynk_ts node` (Decision A).
- The result of a store arm is still printed to text at the arm's boundary (the store functions return `Option<String>` until 1A.23).
