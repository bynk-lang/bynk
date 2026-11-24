# 0912 — Property counterexamples print Decimal and Bytes in Bynk's form

- **Status:** Accepted (v0.657.1)

**Context.** `__bynkShow` used `JSON.stringify`, which throws on a Decimal's `bigint` coefficient and fell back to `String(v)`, giving `[object Object]` (#4198). `Instant` and `Duration` are plain numbers at runtime, so they never hit that fallback.

**Decision.** `__bynkShow` renders a Decimal as its literal (`1.10d`, `0d`) and a `Bytes` as `Bytes.fromHex("…")`, wherever it sits in a value. `Instant` and `Duration` still print as their millisecond number, since the untyped printer cannot tell them from an `Int`.

**Consequences.** Every emitted test module embedding the printer changes; comparison failures over `Bytes` read better too.
