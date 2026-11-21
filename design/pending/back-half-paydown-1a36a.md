---
level: patch
changelog: Groundwork for deriving each generated module's imports from its code instead of predicting them; the generated code and its source maps are byte for byte unchanged
probes: import_prediction_fns
---
refactor(emit): shadow-check tree-derived imports, raw_using, scoped binds and the import_prediction_fns probe (1A.36a)

Part of #3737. Part of #3676.

No output changes. `import_shadow.rs` compares, for every module the positive corpus emits, the imports the built tree asks for with the imports the module has, and asserts the first is a subset of the second; its report lists the modules that import a name the tree never uses. `TsStmt::raw_using(text, names)` carries the names a fixed snippet uses; `name_closure` counts them, and the four surviving snippets move onto it. `name_closure` binds a type parameter and a mapped key only inside their own node and walks an object method's return type. `ts_raw_sites_emit` counts `raw_using(` too and still reads 4.

`import_prediction_fns` is a new gated growth probe, set at its first reading of 8 over an allowlist of the six resolved-fact queries #3737 Decision B keeps (`unit_table_uses_emit`, `unit_table_uses_idempotency`, `call_handler_needs_exec`, `commons_uses_emit`, `mentions_decimal`, `mentions_factory_param`). 1A.36b brings it to 0.
