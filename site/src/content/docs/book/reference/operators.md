---
title: Operators & built-ins
---
## Operators

| Operator | Arity | Operands | Result | Notes |
|---|---|---|---|---|
| `+` `-` `*` `/` | binary | `Int` (or `Float`; or `Decimal` for `+` `-` `*`) | `Int` | arithmetic; no `%` (a `%` is the error `bynk.parse.no_remainder_operator`, which points at `Int.rem` and `Int.mod`, [below](#remainder)); `/` on `Int` truncates and on `Float` divides, but is not defined on `Decimal` ([below](#decimal-arithmetic)) |
| `-` | unary | `Int` (or `Decimal`) | `Int` | negation |
| `==` `!=` | binary | same equality-supporting type (compared structurally) | `Bool` | non-associative |
| `<` `<=` `>` `>=` | binary | two operands of the same [`Ord`](/book/reference/types/#ordering-your-own-types) type | `Bool` | comparison; IEEE on `Float` (`NaN < x` is `false`) |
| `&&` `\|\|` | binary | `Bool` | `Bool` | logical |
| `!` | unary | `Bool` | `Bool` | logical not |
| `implies` | binary | `Bool` | `Bool` | logical implication (`P implies Q` ≡ `!P \|\| Q`, directional); lowest precedence; in invariant predicates ([invariants](/book/reference/agent-invariants/)) |
| `is` | binary | sum value + pattern | `Bool` | variant test, may bind ([guide](/book/guides/type-system/narrow-with-is/)) |
| `?` | postfix | `Result` | unwraps `Ok` | propagates `Err`; only in a `Result`-returning fn |
| `<-` | bind | `Effect[T]` | `T` | sequences an effect in a `let` |
| `~>` | prefix | `Effect[()]` | statement | **deprecated** asynchronous send, rewritten by `bynk check --fix` to `do e.detach()`; restricted to `Effect[()]`, and not allowed in an agent handler ([guide](/book/guides/effects-and-capabilities/synchronous-and-asynchronous-sends/)) |

There is no string concatenation operator: `+` requires numeric operands.

## Precedence

Lowest to highest:

1. `expect` (in expression position)
2. `implies` *(lowest-precedence operator; predicate language)*
3. `||`
4. `&&`
5. `==` `!=` `is` *(non-associative — no chaining)*
6. `<` `<=` `>` `>=`
7. `+` `-`
8. `*` `/`
9. unary `-` `!`
10. postfix: `?`, `.field`, `.method(…)`, calls

So `expect x == 1` parses as `expect (x == 1)`, and `a + b * c` as `a + (b * c)`.

## Remainder {#remainder}

Bynk has no `%` operator. Writing one is the error `bynk.parse.no_remainder_operator`,
which names the two methods that do the work (`Int.rem` and `Int.mod`, ADR 0660):

| Write | Sign of the result | Same as |
|---|---|---|
| `a.rem(b)` | the dividend's (`(-7).rem(2)` is `-1`) | C, Java and JavaScript's `%` |
| `a.mod(b)` | the divisor's (`(-7).mod(2)` is `1`) | Python's `%` |

When both operands are plain names or integer literals, the error offers a quick fix (never applied automatically, since only `Int` has `rem`) that
rewrites `n % 2` to `n.rem(2)`; check that you wanted the dividend's sign, and change
it to `.mod` for a cyclic index. Any other operand, such as `(a + b) % size`, gets the
explanation without a fix.

## Decimal arithmetic

`Decimal` is exact base-10 arithmetic. Its operand and result rules:

| Expression | Result | Notes |
|---|---|---|
| `Decimal + Decimal`, `Decimal - Decimal` | `Decimal` | exact; the result has the larger of the two scales |
| `Decimal * Decimal` | `Decimal` | exact; the result's scale is the sum of the two |
| `-Decimal` | `Decimal` | exact |
| `Decimal / Decimal` | rejected | `bynk.types.decimal_division_unscaled`: a quotient has no exact answer, so write `a.div(b, scale, RoundingMode.<mode>)` |
| `Decimal == Decimal`, `<`, `<=`, `>`, `>=` | `Bool` | by numeric value, so `1.20d == 1.2d` |
| `Decimal` with `Int` or `Float`, in any operator | rejected | `bynk.types.no_numeric_coercion`; use `Decimal.fromInt(n)` or a decimal literal |

See [Decimal](/book/reference/types/#decimal) for rounding, parsing and the wire
form.

## Duration & Instant arithmetic

`Duration` (a span) and `Instant` (an absolute point in time) compose through a
fixed operator surface; every other numeric mix between them — or with `Int` — is a
`bynk.types.no_numeric_coercion` error.

| Expression | Result | Notes |
|---|---|---|
| `Duration ± Duration` | `Duration` | subtraction is unclamped (may go negative) |
| `Duration * Int` / `Int * Duration` | `Duration` | scalar scaling |
| `Duration` vs `Duration` (`<` `<=` `>` `>=`) | `Bool` | comparison |
| `Instant ± Duration` | `Instant` | advance / retreat |
| `Instant - Instant` | `Duration` | the span between |
| `Instant` vs `Instant` (`<` `<=` `>` `>=`) | `Bool` | chronological ordering |

`Instant` is **orderable** (so `sortBy`/`min`/`max` key on it) but **not numeric**
(`sum`/`average` reject it). Timestamp math goes through `Instant` —
`Clock.now() + 5.minutes` is `Instant + Duration`. The earlier `Int + Duration ->
Int` clock-math coercion was **withdrawn** at v0.90; mixing `Instant` with `Int`
now errors. Convert explicitly with `d.toMillis()` / `Duration.millis(n)` and
`t.toEpochMillis()` / `Instant.fromEpochMillis(n)`, and to RFC 3339 text through
`t.toIso()` / `Instant.fromIso(s)`. See [types](/book/reference/types/#duration).

## Other expression forms

| Form | Meaning |
|---|---|
| `if c { … } else { … }` | conditional expression; branches share a type |
| `match e { … }` | exhaustive pattern match ([types](/book/reference/types/#matching)) |
| `let x = e` / `let x: T = e` | binding; `let x <- e` binds an effect |
| `T.of(…)` | refined/opaque checked construction; `T.unsafe(…)` is opaque-only ([refined types](/book/reference/refined-types/)) |
| `name := e` | write a `store Cell` field; committed at handler end ([agents](/book/reference/agents/)) |

## Built-in types

`Int`, `Float`, `String`, `Bool`, `Duration`, `Instant`, the unit `()`, and the
generics `Result[T, E]`, `Option[T]`, `Effect[T]`, `HttpResult[T]`, `Stream[T]`,
`Query[T]`. See the [type system reference](/book/reference/types/).
