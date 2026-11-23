---
level: patch
changelog: Internal restructuring: one spelling for each emitter node builder, and `param` becomes `TsParam::new`; no output changes
probes: node_builder_defs
---
refactor: one spelling for each emitter node builder (1B.4b)

Closes #4051. Part of #3676.

No output changes. The 68 `as` renames of `ts_build` items in 19 production files, and their call sites, are renamed to the canonical names, one commit per builder; the two `ident` locals in `http_client/route.rs` are renamed first so they cannot shadow the builder. `ts_build::param` is deleted and its 115 callers use `TsParam::new` (36 gain `Some(..)` around a bare `TsType`). The four test-module copies of `ident` and 20 shape-equal local builder closures are deleted. `node_builder_defs` falls from 68 to 0 and stays gated, as a guard against a new local wrapper or an `as` rename in a Phase 2 split.
