---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: The `List` kernel's lowering moves into its own module and its expression-bodied operations are built as TypeScript nodes rather than text; no change to emitted code beyond indentation
---
refactor(emit): the list kernel moves into lower_list.rs and its expression arms lower to nodes (1A.18a)

Part of #3719. Part of #3676.

**Commit 1, the move (Decision B).** `lower_list_kernel` and its helpers (`list_ok_elem_ts`, `join_other_elem_ts`, `kernel_key_comparator`, `with_comparator`, `key_is_plain`, `key_fn_is_plain`, `key_fn_ret`) move to `bynk-emit/src/emitter/lower_list.rs`. The 458-line function is a dispatcher of about 90 lines over per-operation builders, and its ceiling entry is gone. Goldens were byte-identical at this commit. The new file is 1,097 production lines (the limit is 2,000).

**Commit 2, nodes (Decision C).** 28 expression-bodied arms build nodes through `node_or_text`, and otherwise give today's text byte for byte: length/count, get, prepend/append/concat, map/filter/flatMap/any/all, take/skip, upTo, first/firstOrElse, sum (Int, Float, Decimal), plain and keyed `distinct`, keyed `distinctBy`/`joinOn`/`leftJoin`/`groupBy`, `parTraverseAll`, and the comparator forms of `sortBy`/`sortByDescending`.
- New `bynk_ts` node `TsExpr::Spread`, an array item printing `...x`, with printer tests.
- `with_comparator` matches the node comparator; `kernel_key_comparator` returns a node and keeps its place, so `note_cmp` fires in the same order. `QueryShape.key_cmp` is a node too.
- The kernel dispatch returns `Built`; the other kernel families stay text there, marked `// 1A:`.
- Shared builders (comparator binding, array-typed params, keyed-helper calls, call/IIFE helpers) are in `lower_list.rs` for the sibling-kernel slices. The track gains a row for each sibling family and for 1A.18b, and the catch-all row is relabelled 1A.20–n.

**Still text (`// 1A:`), 1A.18b.** The statement-bodied arms, including the plain-key sorts.

**Note for 1A.18b.** `fold` and `foldEff` annotate `__f: (acc: A, x: T) => A`, but `TsType::Fn` prints its parameters as `a0`, `a1`, so the annotation cannot be a node byte for byte. 1A.18b needs a decision (a named-parameter function type, or accepting the rename).

**Goldens, as ruled.** 39 `.ts` files, 42 hunks, whitespace only (`git diff -w` is empty); group (c) and (d) are empty.
- 36 stdlib `bynk/list.ts` copies: `traverse`'s lambda body prints 2 columns shallower inside the still-text `foldEff`. Accepted as shallower until 1A.18b.
- `2712_record_pattern_lambda_params` (4 hunks), `2644_ord_recursion_behaviour` (1) and `2690_generic_field_defaults` (1): a lambda or `match` IIFE over a node now prints at its real depth.

**Departures from the proposal.** An IIFE's callee is an `Arrow` node directly, not `Paren(Arrow)`: the printer already brackets an arrow callee, with the same output. `Spread` prints its operand without brackets, matching the old text.

**Gated probes** (against the state after #3762):

| Probe | Before | After |
|---|---|---|
| `ts_writes` | 1407 | 1395 |
| `lowering_text_bridge` | 88 | 86 |
| `ts_any` | 39 | 39 |
| `ts_raw_sites_emit` | 35 | 35 |
| `ts_text_idents` | 14 | 14 |
| `stmt_text_lines` | 30 | 30 |
