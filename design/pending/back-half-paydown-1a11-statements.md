---
level: patch
probes: stmt_text_lines, ts_writes
changelog: Statements and the if tail in a function or handler body are built as typed nodes where their expressions are nodes, with no change to the generated output or its source maps
---
refactor(emit): statements and the `if` tail lower to nodes (1A.11)

`stmt_text_lines` falls from 67 to 59 and `ts_writes` from 1482 to 1477 as the ordinary statement producers (`let`, `let <-`, `do`, assignment, the returning tail, the ternary tail and the `if` tail) build `Item::Node` instead of `Item::Line` when every part is a node; the remaining `Item::Line` sites are the test-domain `expect` and `advance`, `par`, `match`, the `TailSlot` form, the `is`-binding lines and the text fallbacks. No other gated probe moves: `lowering_text_bridge` stays at 111 (the ternary tail reuses the value-position ternary's one text exit), and `ts_raw_sites` and `ts_text_idents` do not change.
