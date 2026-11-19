---
level: patch
probes: ts_any
raises: ts_any — the list kernel's two `any` fallbacks (a checked `traverseTry` shape and a `sequenceAll`/`sequenceTry` shape that are absent) were the text `any`, which the probe does not see; they are now one `TsType::named("any")` in `lower_list.rs` (`any_ty`), kept at the same points as #3719 Decision D requires, and fall when the `1A.fallback` slice removes them
changelog: The `List` kernel's statement-bodied operations are built as TypeScript nodes rather than text, so each prints as a multi-line arrow function; no change to what the emitted code does
---
refactor(emit): the list kernel's statement-bodied arms lower to nodes (1A.18b)

Closes #3719. Part of #3676.

**Goldens, as ruled.** 63 `.ts` files, layout only (36 stdlib `bynk/list.ts` copies and 27 fixture modules); groups (a) re-indentation and (c) one-liners split into multi-line arrows. Group (d) is empty: with the sort `const` split undone, the whitespace-stripped text of every file is identical, so no bracket, brace, string or source-map change.
- Group (c): about 348 one-liner lines split; 6 multi-declarator `const` splits in 3 sort fixtures (217_list_query_aggregates, 227_instant_primitive, 2805_sort_by_descending_list_behaviour). No braces gained, no type brackets dropped.
- Group (a): about 837 re-indented lines, about 828 of them in the stdlib copies, where the `traverse` lambda inside `foldEff` now nests properly.
- The `ts_any` rise from 39 to 40 is accepted (the `any` fallbacks, now one `any_ty()` node).

**Departures.** `TsType::Fn` gains `names` (parallel field) so `fold`'s annotation stays byte-identical; `TsExpr::AddAssign` is added for `average`'s `__s += …`.
