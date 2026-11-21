---
level: patch
changelog: Lowering builds the generic-function-as-value closure, the `Form.decode` and `Form.encode` call and the hoisted async-tail `if` as typed nodes, and nothing in lowering builds text any more; the generated code is unchanged apart from layout, and a lowering step that lacks a checker fact for a receiver namespace or a store operation is reported as an internal fault instead of emitting a placeholder
probes: lowering_text_bridge, stmt_text_lines, ts_writes, ts_any, ts_raw_sites_emit, verbatim_origins, verbatim_sites
raises: verbatim_sites — the `Form.decode`/`Form.encode` field-spec table is `workers_entry` source text for the router's own array literal, so it is one foreign-text carrier (`VerbatimOrigin::FormFieldSpecs`, built at the single site `form_field_specs`) (#3736)
---
refactor(emit): the foreign text leaves become named carriers and the pass-through lines go (#3736, part C)

Part of #3736. Part of #3676.

Nothing in lowering builds text any more. The remaining text is the bridge machinery itself, which 1A.35b deletes.

**Converted to nodes.** The generic-function-as-value closure (`lower_ident`: a `Paren(Arrow)` over typed parameters, the call and the witness nodes), the `Form.decode` decoder (its parse step is `parsed_result_step`, as `Yaml.decode`'s is) and the `Form.encode` call, the hoisted `if` in async-tail position (the slot's type is a `TsType` union), `hoist_value_as_statement`'s text fallback, `par`'s thunk fallback, the `is`-binding and `match` scrutinee paths (`PatPath` is a node, `pattern_tests` and `and_join` build `TsExpr`s, the or-pattern dispatch is one `TsStmt`), the `expect`, `snapshot`, `advance`, `do`, assignment and `let` statement items, the value-position `if` and `match` IIFEs, the `?` guard, the cross-context call and cast, the cookie, query and form call-site serialisers, the `Json` codec statics, the `Query` kernel's `built` (its text halves and `with_comparator`) and the `max`/`min` list kernel.

**The one new carrier.** `VerbatimOrigin::FormFieldSpecs` holds the `__formDecode`/`__formString` field-spec table, which `workers_entry::field_spec_source` returns as source text. It is built at the single site `form_field_specs` in `lower_call/codec_statics.rs`, has a doc comment and a printer test. `verbatim_sites` rises 13 to 14 and `verbatim_origins` 2 to 3.

**Internal faults, not placeholders.** An instance call whose receiver has no recorded type (`/* unknown */`) and a `Set`, `Cache` or `Cell` store operation the checker should never have admitted (`/* unsupported … op */ undefined`) now record a `bynk.internal.missing_checker_fact` fault under 1A.34's policy. `LoweredExpr::into_node`, `LoweredStmt::into_stmt` and `Body::into_nodes` are temporary bridge helpers with the same policy for a value that is still text; they go with the `Text` variant in 1A.35b. A `Set.remove` forced to text by `force_text` is now a fault, so the tests of the text fallbacks are deleted, and one test pins the fault.

**Left for 1A.35b.** `lowering_text_bridge` 34 to 5 and `stmt_text_lines` 23 to 2, all definitions: `From<Built>`'s `Err` arm, `Lowered::bare` (`cfg(test)`), `Pre::push_text` (`cfg(test)`), `Pre::finish_text` and `Pre::finish_built`'s `Err` arm, `LoweredStmt::item`'s `Line` arm, and the `Item::line` helper. The `Item::line` wrappers in `emit.rs` (the result closure, the contract-guard closure, the event guards and `emit_suite_fn`) are text by design and belong to the separate 1A.ws PR. `deserialise_expr`'s text input and the migration step function are 1A.38's. `node_or_text`, `Built` and their text closures remain at their call sites.

Also: `drain_internal_faults` (`bynk-driver` `tests_emit.rs`) reports whether it added errors and the module's output is dropped when it did; a snapshot name is always a string-literal node.

`lowering_text_bridge` falls 34 to 5, `stmt_text_lines` 23 to 2, `ts_writes` and `ts_any` and `ts_raw_sites_emit` fall, and `verbatim_sites` and `verbatim_origins` rise by the one carrier.
