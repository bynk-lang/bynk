---
level: patch
changelog: Lowering builds labelled loops and blocks, while loops, multi-declarator lets, sequence expressions, generators with yield, for-await, array-pattern parameters, the claim-scope declaration, the witness claims and the regex and stream helpers as typed nodes instead of text; the generated code is unchanged apart from layout (and one multi-declarator const split in two), and the source maps of the changed files gain finer statement anchors
probes: lowering_text_bridge, stmt_text_lines, ts_writes
---
refactor(emit): new bynk_ts nodes for the remaining text leaves (#3736, part B)

Part of #3736. Part of #3676.

New `bynk_ts` nodes: `While`, `Labelled`, a labelled `Continue`/`Break`, `LetList`, `ForOf.is_await`, `Sequence`, `OptionalCall`, `Function`, `Yield`, `PreIncrement` and `TsParam.array_pattern`. Each prints through its own helper, is walked by `name_closure` and `await_scan`, and has printer tests; `render_unary`, `render_conditional` and `render_switch` were extracted to stay under the function-length ceilings. `render_labelled` now records its statements' spans.

`lowering_text_bridge` falls 37 to 34, `stmt_text_lines` 25 to 23 and `ts_writes` 1151 to 1143.

Rulings recorded: the 18 changed goldens are blessed (16 layout only; 353 splits `const __ka = …, __kb = …;` into two `const`s, as ruled in 1A.18b; 3770's `textOperand` double bracket is removed). The finer source maps are accepted: more anchors, and some statements in 1750 and 1668 now map to their own source lines. The witness-claims driver change (`SystemHttpClaims.actors` as structured data) stays in this PR.
