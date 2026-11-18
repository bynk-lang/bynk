---
level: patch
probes: stmt_text_lines
changelog: The statement emitter builds a list of body items and renders it through one adapter, with no change to the generated output or its source maps
---
refactor(emit): the statement emitter returns a body (1A.10)

`stmt_text_lines` is new, gated at its baseline of 67 `Item::Line` construction sites, and falls to 0 as 1A.11-1A.13 convert the statement producers to nodes. No other gated probe moves: `ts_writes` stays at 1482, and `lowering_text_bridge`, `ts_raw_sites` and `ts_text_idents` do not change.
