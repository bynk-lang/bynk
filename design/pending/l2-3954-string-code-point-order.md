---
level: minor
changelog: `String` orders and its length predicates count by code point, agreeing with storage, SQL and OpenAPI
breaking: `String` `<`, `sortBy`, `min`, `max` and `Ord` now order by code point, not UTF-16 code unit, so text mixing a character above U+FFFF with one in U+E000..U+FFFF sorts differently; `MinLength`, `MaxLength` and `Length` now count code points at run time too (an emoji counts once). No diagnostic; no fix-it is needed for BMP-only text.
---

## ADR: string-code-point-order
title: String orders and measures by code point everywhere
summary: One comparator, code point order, shared by operators, kernels, Ord witnesses and the storage overlay; predicates count code points

**Context.** `String` ordered by UTF-16 code unit (JavaScript `<`) while storage, D1/SQLite and the length predicates' compile-time check used code points or UTF-8 bytes. Run-time length predicates counted code units, so an astral literal was admitted at compile time and rejected by `.of` (#3949).

**Decision.** Code point order everywhere (#3954). `__bynkCmpString` compares code units and fixes up surrogates at the first difference; `utf8Compare` is that function. The `<`, `<=`, `>`, `>=` operators on `String` and the sort/min/max kernels lower through it. Emitted `MinLength`/`MaxLength`/`Length` checks call `__bynkCodePointLength`. `s.length()` keeps counting UTF-16 code units. Locale collation stays out of scope.

**Consequences.** Mixed astral/U+E000–U+FFFF data sorts differently; BMP-only text is unchanged. The order matches storage and SQL.
