# 0695 — Bitwise operations on Int as numeric kernel methods over the full safe range

- **Status:** Accepted (v0.477)

**Context.** `Int` had arithmetic and, since K6b (#2598), `div`/`rem`/`mod`/`pow`,
but no bitwise operations, so flags, bit-packed ids, hash mixing and binary
protocol work could not be written. JS's `&`, `|`, `^`, `<<` and `>>` truncate
to 32-bit signed integers, while a Bynk `Int` is a 53-bit safe integer (spec
§5.2); lowering naively would silently corrupt any value from 2^31.

**Decision.**

- **A. Methods, not operators.** As K6b did: no grammar change, no new
  precedence levels, no clash between `|` and or-patterns. Operators remain
  possible later as sugar over these.
- **B. The full 53-bit range.** Each operand is split into a high part (below
  2^21) and a low 32 bits, the native operator is applied to each half, and the
  halves are recombined, so the result is exact for every safe integer. Operands
  below 2^31 take the native fast path. These are slower than JS's own 32-bit
  operators on large values, and that is the documented cost.
- **C. Non-negative operands only.** Two's complement on a sign-magnitude
  domain has no natural meaning, and flags and packing are non-negative, so a
  negative operand faults. `not` is omitted (its width would be arbitrary);
  `a.andNot(mask)` clears the bits of `mask` instead.
- **D. Shifts.** `shl` and `shr` take a count in `0..52` and fault outside it;
  `shl` faults if the result leaves the safe range (as `pow` does), while `shr`
  floors. Both are exact (a power-of-two scaling is exact in a double).
- **E. Runtime helpers.** `__bynkIntAnd`, `__bynkIntOr`, `__bynkIntXor`,
  `__bynkIntAndNot`, `__bynkIntShl` and `__bynkIntShr` sit in `int.ts` with the
  other `Int` helpers, so their fault branches stay out of coverage.
- **F. Stable.** `ushr` is not offered: with non-negative operands `shr` is
  already the logical shift.

**Consequences.** Flags, packed ids and masks are one call. Hashing interop
that expects 32-bit wraparound semantics is out of scope; a `Bytes`-level API
is the right tool. Operators-as-sugar is a named possible follow-on.
