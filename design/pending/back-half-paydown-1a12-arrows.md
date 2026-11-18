---
level: patch
changelog: The code generator builds value-position `if`, block and lambda bodies as arrow nodes where it can, printing them at their real nesting depth, with no change to behaviour or source maps
---
refactor(emit): value-position `if`, blocks and lambdas lower to arrow nodes (1A.12)

The `if` IIFE, `lower_block_as_expr` and `lower_lambda` (block and expression bodies) build `TsExpr::Arrow` nodes when the whole body, the condition and the parameters are nodes, and otherwise emit today's text byte for byte. A new `TsArrowBody::Multiline` prints `{`, one statement per line one level deeper than the line the arrow starts on, and `}` back at that indent; the printer threads a depth through expression rendering, brackets an object-literal expression body, and records no source-map checkpoints inside such a body, so the `.map` fixtures are unchanged. `Body::to_nodes_unmapped` drops the `Mark`s for these bodies, which are lowered under `without_source_map` today. The `match` IIFE stays text (1A.13).

Layout of IIFE and lambda bodies changes where the arrow is now a node: a body prints at its real depth (the deferred half of #3708 Decision B), and a lambda whose expression body hoists statements prints them at their depth rather than at column 0. An arrow still embedded in a text parent prints at depth 0. Four positive fixtures change layout only.

No gated probe moved: the text fallbacks keep their construction sites, so `ts_writes`, `lowering_text_bridge` and `stmt_text_lines` read as before until the bridge is deleted in 1A.35.
