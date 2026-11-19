---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: The `Json`, `Yaml` and `Form` codec calls and the typed decoders are built as TypeScript nodes rather than text, so a typed decoder is laid out as an indented block; no change to what the emitted code does
---
refactor(emit): codec statics and typed decoders lower to nodes; the serialiser takes a node; `TryCatch` gains `finally` (1A.21)

Closes #3722. Part of #3676.

**What changed.** `json_decoder_fn` builds a typed `Arrow` with a `Multiline` body (`Let`, `TryCatch`, `Const`, `If`, `Return`, and the `Err({ kind, path, message })` tail with a template-literal `message`); `sql_bool_pass` and `pg_row_pass` return statements; `Json.decode`, `decodeSql`, `Yaml.decode`, `Json.encode`, `Json.encodePlain` and `Form.encode` build nodes, and `lower_json_codec_call` returns `Option<Built>`. The serialiser entry points (`serialise_expr`, `serialise_expr_via` and the value leaf) take `&TsExpr` (the `1A.ser` row, closed here); `bynk_ts` gains `finally` and an optional `catch` on `TryCatch` (reused by 1A.32), `expr_contains_await`, and a `debug_assert` and doc on `TsExpr::TypeApp` (from the 1A.20 review). The codec statics moved first, mechanically, to `lower_call/codec_statics.rs`.

**Goldens, as ruled.** See the PR description for the counts and groups.

**Probes.** `ts_writes` and `lowering_text_bridge` fall (1377 to 1371 and 68 to 62 against 1A.20). `ts_text_idents`, `ts_any` and `ast_importers` do not move.

**Departures and named leftovers.**
- `Json.encodePlain`'s argument is now the call argument of the arrow (`((__pv): string => { … })(arg)`), not the default of its parameter (`TsParam` has no default); the argument is still evaluated before the body raises the flag. A text argument keeps the old default-parameter form.
- A `Form.decode` keeps its decoder as text, and a `Form.encode` over a record with `@name` fields keeps its call as text: the field-spec table is `workers_entry` source text.
- A serialiser value that is still text is wrapped by `text_value` (marked `// 1A: serialiser value is text`); the snapshot `expect`'s known-type arm stays text.
