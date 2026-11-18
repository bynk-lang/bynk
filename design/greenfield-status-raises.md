<!-- GENERATED, APPEND-ONLY — written by `cargo xtask stamp --apply` (xtask/src/stamp.rs) at merge, one row per growth probe an increment raises with its `raises` frontmatter (#3679).
     `from` is the probe's committed leading count before the stamp and `to` the live count the stamp commits; a PR that moves no growth probe upward writes nothing here. Do not hand-edit. -->

# Growth-probe raises

| Probe | From | To | Version | PR | Reason |
|---|---|---|---|---|---|
| ts_any | 38 | 39 | v0.639.28 | #3762 | the node form of the test-scaffold refined brand (`unchecked_construct_test_node`, `TsType::named("any")`) sits beside the text `unchecked_construct_test`, which the driver's `property_generators.rs` still calls; the cast itself is unchanged (P7.2) and the count falls when those callers convert |
