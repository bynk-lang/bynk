---
level: patch
changelog: The serialisation codecs, the Worker group entry, the email handler and the extra imports are built as trees, not text, and the codec guards that sat one level shallow now sit at their true depth; only indentation in the generated code changes, and its source maps are unchanged
probes: ts_writes, ts_raw_sites_emit, emit_print_calls
---
refactor(emit): the serialisation, group, email and extra-import raw sites become nodes (1A.38a)

Part of #3745. Part of #3676.

Eight `TsStmt::raw(` sites in `bynk-emit` become nodes: the codec field guards and the generic-sum `kind` cascade, the two refined-type `.of` probes, the codec factory's arrow constants, the group `index.ts`, the email handler body and the driver-built extra imports (`ExtraImport`, owned by `bynk-emit`). The four fixed authored snippets stay as text with their reason beside them. `bynk_ts` gains `TsExpr::MultilineConditional`, a `named` list on `TsDecl::ImportDefault`, and prints an `else if` link at the chain's own depth.

`ts_raw_sites_emit` falls by 8 (17 to 9: serialisation 5, group 1, email 1, extra imports 1). `ts_writes` falls from 784 to 772 and `emit_print_calls` from 62 to 55 with the deleted `format!`, `push_str` and print-and-splice code.
