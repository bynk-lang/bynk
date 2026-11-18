---
level: patch
probes: ast_importers
changelog: The statement emitter moves out of `lower.rs` into `emitter/lower_stmt.rs`, with no change to the generated output
---
refactor(emit): move the statement emitter out of lower.rs (1A.9)

`ast_importers` rises from 13 to 14: `lower_stmt.rs` imports `bynk_syntax::ast` for the statement forms it now owns. This is a split of an existing importer (`lower.rs`), not new reach into the AST, and the AST types are not re-exported through `lower.rs` to hide it. No other probe moves.
