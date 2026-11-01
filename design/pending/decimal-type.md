---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: A built-in exact `Decimal` base type for money and fractional arithmetic: `1.20d` literals, exact `+`/`-`/`*`, `div` and `round` with a stated scale and rounding mode, ordering by value, and a canonical JSON string on the wire
---

## ADR: decimal-base-type
title: A built-in exact Decimal base type
summary: Decimal is an arbitrary-precision base-10 base type with a kept scale, no `/`, a `d` literal, value equality and a JSON-string wire form

**Context.** The only exact numeric type was `Int`, a 53-bit safe integer. `Float`
is IEEE 754, and the specification itself warned `0.1 + 0.2 != 0.3` and advised
working in `Int` units. Every service handling prices, tax or currency
conversion re-implemented scaled-integer arithmetic, rounding, formatting and a
JSON shape by hand, and a JSON `number` price is parsed as a float by most
clients and loses precision past 2^53 minor units. Refinements such as
`Price = Float where Positive` steered authors to the wrong base type
(#2622).

**Decision.** `Decimal` is a built-in base type, the layer-0 option (A), so it can
be refined, admitted from a literal, compared and keyed as a base type:

- **Representation (B).** An arbitrary-precision integer *coefficient* and a
  non-negative integer *scale*; the value is `coefficient / 10^scale`. At runtime
  a frozen `{ coefficient: bigint, scale: number }`, written structurally in the
  emitted types so no module needs a type import. `+`, `-`, `*` and unary `-` are
  exact (the result scale is the larger scale for `+`/`-`, the sum for `*`).
  `/` is **not defined**: `bynk.types.decimal_division_unscaled` points at
  `a.div(b, scale, mode)`, which states the quotient's scale and rounding mode, so
  cost and rounding stay visible. A zero divisor faults, as `Int` division does.
  `d.round(scale, mode)` pads when the scale grows and rounds when it shrinks. The
  scale of `round` and `div` is capped at 1000, in line with the 1000-character
  parse cap: above it (or below zero) the call faults, so a hostile scale cannot
  allocate a huge `BigInt`.
  The modes are `Down`, `Up`, `Floor`, `Ceiling`, `HalfUp`, `HalfDown`, `HalfEven`.
- **Literals (C).** A `d` suffix: `1.20d`, `5d`, `0.005d`, with `_` separators and
  no exponent. The scale is the number of fraction digits as written. No `Int` or
  `Float` literal is ever a `Decimal`, and the two never mix
  (`bynk.types.no_numeric_coercion`); the conversions are explicit
  (`Decimal.fromInt`, `toInt`, `toFloat`).
- **Wire (D).** A JSON string of the canonical text, `"1.20"`. Decode is strict
  like `Int.parse`: an optional `-`, digits, optionally `.` and digits, at most
  1000 characters; an exponent, a leading `+`, whitespace, `.5`, `5.` or a JSON
  number is a `StructuralMismatch` at the field path, and a refined type then
  applies its predicates to the decoded value (`RefinementViolation`), and
  reports the offending value as its canonical text. The wire decoder also
  refuses a leading zero (`"007"`, `"-01.5"`; `"0.5"` is fine) so the form is
  canonical, whereas the in-language `Decimal.parse` stays lenient about it. The
  scale survives the round trip. The encoding is part of a persisted agent's stored
  state, so it is final at first ship (Gate 3).
- **Equality and order (E).** By numeric value, so `1.20d == 1.2d`; `scale()` and
  `toString()` still tell them apart. A `Decimal` is `Eq`, `Ord` and `Key`: a
  `Map` key or `Set` member holds `1.20d` and `1.2d` as one key, via the value's
  canonical text in the runtime's key encoder.
- **Currency (F).** Out of scope for the type. A `bynk.money` commons pairing a
  `Decimal` with an ISO 4217 code is a follow-on.

Refinements: `Positive`, `NonNegative` and `InRange` apply, with `Decimal`
literals as `InRange` bounds (`PredKind::InRangeD`); a bound of another numeric
type is `no_numeric_coercion`. The contract canonical form writes bounds by value
(`InRangeD(0, 1)` for `0.00d`, `1.00d`), so a changed scale in a bound is not a
contract change.

**Rounding modes are syntactic.** A mode is written `RoundingMode.HalfEven` and read
at the `round`/`div` argument, as an `ErrorStatus` is read at `RawError`; it is
not a first-class value. That keeps the kernel signatures free of a new built-in
enum type, at the cost that a mode cannot be passed through a function
parameter. A first-class `RoundingMode` is a compatible later widening, since
every accepted program stays accepted. The mode used anywhere else is
`bynk.types.rounding_mode_position`, and a non-mode argument is
`bynk.types.unknown_rounding_mode`.

**Consequences.** The runtime gains a `decimal` module over `BigInt` (not part of
ADR 0310's published ABI). The structural `==` walker, the `Key` encoder and the
comparator witnesses recognise a decimal by its `bigint` coefficient. A bare
`Decimal` has no zero value, so an agent state field of that type needs an
initialiser. Parsing is capped at 1000 characters so a request body cannot make
`BigInt` parsing a denial of service. A `Decimal` in a SQL row (`Json.decodeSql`) is
not admitted, and `average` and a `Query` aggregate over a `Decimal` are not yet
provided; `List.sum` is. The Book gains a "Handle money correctly" guide that
replaces the "work in `Int` units" advice; a `Decimal` interpolates into a string as its canonical text, is admitted as a `const` and as a record-field default (written on the wire as its string), and a failed `expect` prints it as `1.10d`; hover, completion
and signature help cover the kernel, and the tree-sitter and TextMate grammars lex `1.20d`.
