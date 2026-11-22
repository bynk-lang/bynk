---
level: patch
changelog: Internal restructuring: a record field's wire default is built from its `DefaultValue` tree and the empty `deps` object prints as `{}`; no behaviour changes
---
refactor: wire defaults are rendered from `DefaultValue`; `empty_deps` is an empty object (1A.40b)

Part of #3996. Part of #3676.

No behaviour changes. The wire form of a field default is `wire_default_node(&DefaultValue)`, a node built beside `default_value_node` in `bynk-emit`, and `bynk-check` stops writing TypeScript text for it: `lower_field_default_wire`, `lower_field_default_wire_in` and `lower_event_field_default_wire` return `Result<(), DefaultReason>` with every message unchanged, `lower_base_literal` and the `format!` arms are deleted, and a test pins that the validator and `field_default_value` agree on every default. `compose::empty_deps` is an empty object. The audit loses its `rendered literal` class.

Golden class (ruled in #3996, Decision C): `const deps = {  };` becomes `const deps = {};` in 209 `compose.ts` goldens (212 lines, 183 `deps` and 29 `<unit>Deps`), token-identical, on both targets. The test-harness twin, `testgen/test_deps.rs:350`, stays with 1B.5.

No gated probe moves: the deleted `format!` lines were in `bynk-check`, outside `ts_writes`'s universe. `node_builder_defs` stays at 68 (`empty_deps` has no parameters, so it is a constant, which the probe now says in its doc and tests).
