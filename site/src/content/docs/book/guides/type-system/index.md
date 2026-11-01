---
title: The type system
---
Bynk makes illegal states unrepresentable: refined and opaque types, sums and
records, and `Result`/`Option` errors-as-values, all checked before emission.

**Understand**
- [The type-system philosophy](/book/guides/type-system/philosophy/)
- [The refined-literal admission model](/book/guides/type-system/refined-literal-admission/)

**Do**
- [Define and validate untrusted input](/book/guides/type-system/define-and-validate/)
- [Decode untrusted JSON into a typed value](/book/guides/type-system/decode-json/)
- [Use a literal where a refined type is expected](/book/guides/type-system/use-a-literal/)
- [Define sum, record, and opaque types](/book/guides/type-system/define-types/)
- [Work with `Result` and optional values](/book/guides/type-system/result-and-optionals/)
- [Pattern-match with `match`](/book/guides/type-system/match/)
- [Match on a record's fields](/book/guides/type-system/match-on-a-records-fields/)
- [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/)
- [Declare `Eq` on a generic that compares](/book/guides/type-system/declare-eq-bounds/)
- [Understand ordering your own types](/book/guides/type-system/ordering-your-own-types/)

**See also:** [Reference — Type system](/book/reference/types/),
[Refined-type API](/book/reference/refined-types/),
[Spec §6 — The type system](/book/spec/type-system/).
