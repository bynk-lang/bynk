---
level: patch
changelog: Internal restructuring: the mock, `expect` and claim-scope lowering move out of `lower.rs` into their own modules
---
refactor: split lower.rs, part two (2.2b)

Part of #4087. Part of #3676.

A pure move: the `Val[T]` mock family (`lower/mock.rs`), the test-domain
expressions (`lower/expect.rs`: `expect`, fault claims, recorded calls, `trace`,
observations) and the Idempotency claim scope with its tests
(`lower/claims.rs`) leave `lower.rs`, which re-exports them, so no caller
changes. Function bodies are byte-identical; output is unchanged.

No gated probe moves: `ast_importers` is unchanged because the three new files
are entered in `AST_IMPORTER_ORIGINS` (origin `emitter/lower.rs`), so no
`probes:` line. `lower.rs` now reads 2,918 production lines; its ceiling in
`xtask/module-line-ceilings.txt` is left alone, as the coordinator ruled.
