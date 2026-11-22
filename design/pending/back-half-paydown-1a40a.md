---
level: patch
changelog: Internal restructuring: the codec emitter builds field subscripts and error paths as expression nodes, not text; no emitted output changes
probes: ts_writes
---
refactor: the serialise family takes `json` and `path` as nodes (1A.40a)

Part of #3996. Part of #3676.

No output changes. The `json` and `path_expr` parameters of the serialise and deserialise family are `TsExpr` rather than `&str`, so `obj["f"]`, `argsObj["f"]` and `` `${path}.f` `` are an `Index` and a `TemplateLit` built by `path_field`, `path_elem` and `path_entry`, and the four text twins (`serialise_field_ident`, `serialise_text`, `serialise_via_text`, `deserialise_node`) are gone. The audit loses its `subscript` and `template path` classes. `ts_writes` falls by 5 (the four `format!` lines that spelled the text, and `argsObj`'s); the probes `node_builder_defs` and the rest do not move.
