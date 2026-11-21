---
level: patch
changelog: The code generator's lowering returns trees only: the text bridge, its constructors, its counters and the string type renderer are deleted, and the generated code and its source maps are byte for byte unchanged
probes: ts_writes, ts_any, verbatim_sites, ts_raw_sites_emit, emit_print_calls
---
refactor(emit): delete the text bridge (1A.35b)

Closes #3736. Part of #3676.

`LoweredExpr`, `LoweredStmt`, `Item::Line`, `Item::Indent`, `Built`, `node_or_text`, the text constructors, `into_text`, `ts_array_elem`, the string `ts_ty`, `TEXT_EXITS`/`TEXT_STMTS`/`LINES_RENDERED`, `force_text` and the `text-exit-counters` feature are deleted. `Lowered` is `{ pre: Vec<TsStmt>, expr: TsExpr }`. The `TsExpr`/`TsType` tree is `PartialEq`, so the comparator recursion key compares witnesses structurally instead of printing them.

`lowering_text_bridge` and `stmt_text_lines` reach 0 and are retired (their rows are deleted from the table by hand, and they are not in `probes:` because a retired probe is not a gated name), which meets the Phase 1 exit row. `emit_print_calls` is the new gated growth probe over the surviving `print_expr(`/`print_stmt(`/`print_type(` boundaries, set at its first reading. `ts_writes`, `ts_any`, `verbatim_sites` and `ts_raw_sites_emit` fall with the deleted code.
