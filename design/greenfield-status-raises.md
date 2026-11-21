<!-- GENERATED, APPEND-ONLY — written by `cargo xtask stamp --apply` (xtask/src/stamp.rs) at merge, one row per growth probe an increment raises with its `raises` frontmatter (#3679).
     `from` is the probe's committed leading count before the stamp and `to` the live count the stamp commits; a PR that moves no growth probe upward writes nothing here. Do not hand-edit. -->

# Growth-probe raises

| Probe | From | To | Version | PR | Reason |
|---|---|---|---|---|---|
| verbatim_sites | 13 | 14 | v0.640.1 |  | the `Form.decode`/`Form.encode` field-spec table is `workers_entry` source text for the router's own array literal, so it is one foreign-text carrier (`VerbatimOrigin::FormFieldSpecs`, built at the single site `form_field_specs`) (#3736) |
| ts_any | 39 | 40 | v0.639.30 | #3764 | the list kernel's two `any` fallbacks (a checked `traverseTry` shape and a `sequenceAll`/`sequenceTry` shape that are absent) were the text `any`, which the probe does not see; they are now one `TsType::named("any")` in `lower_list.rs` (`any_ty`), kept at the same points as #3719 Decision D requires, and fall when the `1A.fallback` slice removes them |
| ts_any | 38 | 39 | v0.639.28 | #3762 | the node form of the test-scaffold refined brand (`unchecked_construct_test_node`, `TsType::named("any")`) sits beside the text `unchecked_construct_test`, which the driver's `property_generators.rs` still calls; the cast itself is unchanged (P7.2) and the count falls when those callers convert |
