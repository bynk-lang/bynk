---
level: patch
changelog: Internal restructuring: the test harness builds the property generators' Decimal values, canonical literals and branded boundaries as typed nodes instead of text, so a zero-field record prints as an empty object; no behaviour changes
probes: testgen_print_calls, ts_writes, ts_any
---
refactor: the property generators build nodes, not text (1B.5b, second PR)

Part of #4054. Part of #3676.

The `Decimal` draw and canonical value (`decimal_node`), the canonical base literals (`base_canon_node`, which replaces the `base_canon` text) and the refined and opaque branding of boundaries, the canonical value and the shrinker (`unchecked_construct_test_node`) are nodes. The `unchecked_construct_test` text twin is deleted. The zero-field record is an empty `Object`, so the `{  }` quirk is gone.

Golden: one line in one file (`1397_property_record_binding_zero_field`) prints `{}` where it printed `{  }`, the layout class ruled in 1A.25 and 1A.40b.

Probes: `testgen_print_calls` 21 to 18, `ts_writes` 521 to 514, `ts_any` 25 to 24; `ts_raw_sites_testgen` stays at 17.
