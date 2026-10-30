---
level: minor
probes: ts_writes
changelog: "`Int` gains `div` (floor), `rem` (the remainder of `/`), `mod` (the remainder of `div`, taking the divisor's sign) and an exact `pow`, each faulting on a zero divisor or a result past the safe range; `Float` gains `sqrt`, `pow`, `exp`, `ln`, `log10`, `log2`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan` and `atan2`, and `Float.pi()`/`Float.e()` (standard-library K6b, [#2598](https://github.com/bynk-lang/bynk/issues/2598))"
---

## ADR: math-kernel
title: Integer division companions and Float functions as numeric kernel methods
summary: `Int` gains `rem` (pairing with truncating `/`), `div`/`mod` (floored, the divisor's sign) and an exact, overflow-faulting `pow`; `Float` gains the `Math` functions, host-defined outside their domains; all as value methods, with `Float.pi()`/`Float.e()` statics, not a `Math` namespace

**Context.** Bynk had no remainder (no `%`, and `BinOp` has no `Mod`), so
bucketing, wrap-around and parity could not be written. There was no square
root, power, logarithm or trigonometry either. ADR 0042 made `Int / Int`
truncate and `Float` arithmetic host-defined; #1657 made `Int` division by
zero a fault. The numeric kernel is value methods (`abs`, `round`, `clamp`,
ADR 0048). An operator is grammar work (ADR 0023). The track sketched
"`Math` statics".

**Decision.**

- **A. Three division companions on `Int`, each faulting on a zero divisor.**
  `rem` is the remainder of `/` and takes the dividend's sign
  (`a == b * (a / b) + a.rem(b)`). `div` is floor division and `mod` its
  remainder, taking the divisor's sign (`a == b * a.div(b) + a.mod(b)`), so
  `x.mod(n)` is in `0..n-1` for a positive `n`. Both pairs, because `/` and
  `rem` match the C-family `%` a port expects, and `div`/`mod` avoid the
  negative-remainder trap; naming both makes the choice visible. A `-0` is
  normalised to `0`.
- **B. `Int.pow` is exact and checked.** Repeated squaring, each step checked
  to stay a safe integer, so a result past ±(2^53 − 1) faults, as does a
  negative exponent (no `Int` result). This is stricter than `*` (#1657 left
  `+ - *` unchecked) because `pow` overflows so readily that an imprecise
  result would be a trap. `0.pow(0) == 1`.
- **C. `Float` functions over JS `Math`.** `sqrt`, `pow`, `exp`, `ln`, `log10`,
  `log2`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, and `y.atan2(x)`,
  radians throughout. Outside a domain the result is host-defined (`NaN`,
  `-Infinity`), as ADR 0042 makes `Float` arithmetic; `isNaN`/`isFinite`
  check.
- **D. Value methods, not a `Math` namespace.** They extend the numeric kernel
  in its own style, chain, and add no namespace a user `type Math` would meet.
  The two constants are statics, `Float.pi()` and `Float.e()`, beside
  `Float.parse`, since a static is a call.
- **E. Runtime helpers for the `Int` operations.** `__bynkIntDivFloor`,
  `__bynkIntRem`, `__bynkIntMod` and `__bynkIntPow` sit in `int.ts` with
  `__bynkIntDiv`, so their fault branches stay out of coverage. The `Float`
  functions lower inline.
- **F. Stable.**

**Consequences.** Remainders, powers and the usual functions are one call.
A `%` or `**` operator remains possible later as grammar work over these. Unary
`-` on a `Float` literal is still refused (`0.0 - 1.0` is the workaround); that
is a separate, pre-existing gap.
