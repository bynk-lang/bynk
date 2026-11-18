---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: Comparator and codec witnesses are built as TypeScript nodes rather than text; no change to emitted code beyond possible layout
---
refactor(emit): comparator and codec witnesses build nodes (1A.16)

Closes #3717. Part of #3676.

`CmpGen` and `CodecGen` return `TsExpr` (`for_ty_node`), and the `LowerCtx` wrappers `comparator`, `codec` and `witness_args` return nodes. `note_cmp`, `note_json_codec` and `note_boundary_codec` fire as before.

This closes two earlier fallbacks:
- 1A.5: an `Ord` comparison over a composite comparator is now `Binary(op, Call(cmp, [l, r]), 0)`.
- 1A.6: a call with comparator or codec witnesses is now a node.

The recursion frame keeps a printed key (`print_expr` of each argument), used only for comparison; nothing printed for it reaches the output.

Still printed for text callers (marked `// 1A: witness printed for a text caller`): the capability call family, the closure/query family, `Json` rigid decode/encode, and the kernel key comparator.

**Goldens.** 2 fixture files change, layout only (`git diff -w` is empty), as ruled: a sum's payload object prints `{ "A": … }` through the printer where the text builder wrote `{"A": …}`. They are `2647_ord_generic_type_behaviour/expected/demo/pairs.ts` and `2648_ord_recursive_type_behaviour/expected/demo/expr.ts`. No std `list.ts` copy changes, and no printer special case was added.

**Gated probes** (against the state after #3757):

| Probe | Before | After |
|---|---|---|
| `ts_writes` | 1428 | 1419 |
| `lowering_text_bridge` | 89 | 88 |
