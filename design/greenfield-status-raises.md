<!-- GENERATED, APPEND-ONLY — written by `cargo xtask stamp --apply` (xtask/src/stamp.rs) at merge, one row per growth probe an increment raises with its `raises` frontmatter (#3679).
     `from` is the probe's committed leading count before the stamp and `to` the live count the stamp commits; a PR that moves no growth probe upward writes nothing here. Do not hand-edit. -->

# Growth-probe raises

| Probe | From | To | Version | PR | Reason |
|---|---|---|---|---|---|
| emit_abi_shapes | 104 | 109 | v0.742.0 | #4406 | the `WorkersR2` binding names five more types (`R2GetIf`, `R2List`, `R2Partial`, `R2Range`, `R2Read`) that the new operations take and return (#4049) |
| ts_writes | 473 | 475 | v0.730.0 | #4376 | two more HttpResult statuses in the runtime's status tables (#3917) |
| ts_writes | 473 | 475 | v0.727.0 | #4367 | the qualifier's namespace text, as for ts_raw_sites_emit (#4045) |
| ts_writes | 471 | 473 | v0.692.0 |  | the security policy literal carries two more typed string fields, the HTML CSP and referrer policy (#3923) |
| ts_writes | 470 | 471 | v0.674.0 | #4249 | the typed headers decoder is built as typed nodes in the entry emitter (#3911) |
| ts_writes | 467 | 470 | v0.670.0 | #4241 | the typed path-parameter decoder and the enum segment of the system driver URL are built as typed nodes in the entry and driver emitters (#3910) |
| ts_writes | 462 | 467 | v0.662.1 |  | the route's `applyChallenge` wrap and the upgrade's challenge array are built as typed nodes in the entry emitter (#3914) |
| ts_writes | 458 | 464 | v0.640.70 |  | 6 `format!` lines of hand-laid JSON (the group inspect listing and agents manifest, which `serde_json::to_string_pretty` wrote before the code moved into `bynk-emit`); they write JSON, not TypeScript |
| ts_writes | 514 | 517 | v0.640.60 | #4070 | the block renderer's three `format!` calls moved from the CLI (`bynk/src/deploy/config.rs`) into `bynk-emit` (1C.2); they write TOML, not TypeScript, and are not new |
| testgen_print_calls | 43 | 44 | v0.640.42 | #4055 | moved from production emission (1B.2b), not new |
| ts_raw_sites_emit | 4 | 6 | v0.640.38 | #4030 | reclassified from ts_raw_sites_testgen: the production commons barrel (sum unchanged) |
| verbatim_sites | 13 | 14 | v0.640.1 |  | the `Form.decode`/`Form.encode` field-spec table is `workers_entry` source text for the router's own array literal, so it is one foreign-text carrier (`VerbatimOrigin::FormFieldSpecs`, built at the single site `form_field_specs`) (#3736) |
| ts_any | 39 | 40 | v0.639.30 | #3764 | the list kernel's two `any` fallbacks (a checked `traverseTry` shape and a `sequenceAll`/`sequenceTry` shape that are absent) were the text `any`, which the probe does not see; they are now one `TsType::named("any")` in `lower_list.rs` (`any_ty`), kept at the same points as #3719 Decision D requires, and fall when the `1A.fallback` slice removes them |
| ts_any | 38 | 39 | v0.639.28 | #3762 | the node form of the test-scaffold refined brand (`unchecked_construct_test_node`, `TsType::named("any")`) sits beside the text `unchecked_construct_test`, which the driver's `property_generators.rs` still calls; the cast itself is unchanged (P7.2) and the count falls when those callers convert |
