---
level: patch
changelog: Lowering builds the typeof and indexed-access casts, tail-call temporaries, or-pattern dispatch and several constructor and snapshot helpers as typed nodes instead of text; the generated code is unchanged apart from layout, and ten source maps gain finer statement anchors
probes: lowering_text_bridge, stmt_text_lines
---
refactor(emit): the first text leaves of lowering become nodes (#3736, part A)

Part of #3736. Part of #3676.

`lowering_text_bridge` falls 43 to 37 and `stmt_text_lines` 30 to 25. Twelve goldens are re-laid out (token-identical). Ten source maps change, all in files whose TypeScript changed: a region that was one text block now gives each inner statement its own anchor (+2 to +10 anchors, and some anchors move to the inner lines).
