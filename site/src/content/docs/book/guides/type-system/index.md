---
title: The type system
---
Bynk makes illegal states unrepresentable: refined and opaque types, sums and
records, and `Result`/`Option` errors-as-values, all checked before emission.

**Understand**
- [The type-system philosophy](/book/concepts/type-system/philosophy/)
- [The refined-literal admission model](/book/concepts/type-system/refined-literal-admission/)
- [Recursion and iteration](/book/concepts/type-system/recursion-and-iteration/)
- [Choosing `Int`, `Float` or `Decimal`](/book/guides/type-system/handle-money-correctly/#choosing-int-float-or-decimal)

**Do**
- [Define and validate untrusted input](/book/guides/type-system/define-and-validate/)
- [Decode untrusted JSON into a typed value](/book/guides/type-system/decode-json/)
- [Read a YAML settings file into a typed value](/book/guides/type-system/decode-yaml/)
- [Use a literal where a refined type is expected](/book/guides/type-system/use-a-literal/)
- [Define sum, record, and opaque types](/book/guides/type-system/define-types/)
- [Work with `Result` and optional values](/book/guides/type-system/result-and-optionals/)
- [Pattern-match with `match`](/book/guides/type-system/match/)
- [Match on a record's fields](/book/guides/type-system/match-on-a-records-fields/)
- [Destructure a record with `let`](/book/guides/type-system/destructure-a-record-with-let/)
- [Match on a list's shape](/book/guides/type-system/match-on-a-lists-shape/)
- [Match on a range of numbers](/book/guides/type-system/match-on-a-range-of-numbers/)
- [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/)
- [Fault on the impossible or the unfinished](/book/guides/type-system/fault-on-the-impossible/)
- [Declare `Eq` on a generic that compares](/book/guides/type-system/declare-eq-bounds/)
- [Recursive generic types](/book/guides/type-system/recursive-generic-types/)
- [Handle money correctly](/book/guides/type-system/handle-money-correctly/)
- [Rename a field safely](/book/guides/type-system/rename-a-field-safely/)
- [Decode a third-party webhook into typed values](/book/guides/type-system/decode-a-third-party-webhook/)
- [Understand ordering your own types](/book/concepts/type-system/ordering-your-own-types/)
- [Generic code that crosses the boundary](/book/guides/type-system/generic-code-that-crosses-the-boundary/)
- [Constructors for generic types](/book/guides/type-system/constructors-for-generic-types/)

**See also:** [Reference — Type system](/reference/types/),
[Refined-type API](/reference/refined-types/),
[Spec §6 — The type system](/reference/spec/type-system/).
