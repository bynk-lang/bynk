---
title: Refined-type API
---
A refined type is a base type plus one or more predicates:

```bynk
type Age = Int where InRange(0, 150)
type Username = String where MinLength(3) && MaxLength(20)
```

Predicates are combined with `&&`. A refined type emits a branded type plus a
constructor object with `.of` — its only runtime constructor (ADR 0182). A value
enters the type through `.of` (checked) or compile-time literal admission; there
is no `.unsafe` escape hatch (that is opaque-only).

## Predicates

### Int

| Predicate | Holds when |
|---|---|
| `NonNegative` | value ≥ 0 |
| `Positive` | value > 0 |
| `InRange(lo, hi)` | lo ≤ value ≤ hi (inclusive) |

These apply to `Int`, `Float` and `Decimal`. The `InRange` bounds are literals of
the base's own type: `0`, `0.0` or `0.00d`.

### String

| Predicate | Holds when |
|---|---|
| `NonEmpty` | length ≥ 1 |
| `MinLength(n)` | length ≥ n |
| `MaxLength(n)` | length ≤ n |
| `Length(n)` | length = n |
| `Matches(regex)` | the whole string matches `regex` (anchored) |

A predicate must apply to the base type (`bynk.types.predicate_base_mismatch`).
An `InRange` with `lo > hi` is rejected (`bynk.types.inverted_range`), as is a set
of predicates that admit no value (`bynk.types.empty_refinement`) or a negative
length (`bynk.types.negative_length`). An invalid regex is
`bynk.types.invalid_regex`. The emitted boundary check runs a `Matches` regex
under the platform's backtracking `RegExp`, on untrusted input, so the compiler
rejects a regex that can match the same text along many paths (a
denial-of-service risk):

- `bynk.types.catastrophic_regex`: exponential ambiguity, such as nested
  repetition (`(a+)+`), overlapping alternatives (`(a|a)+`, `(\d|\d\d)+`) or a
  bounded repeat inside a loop or of an ambiguous body (`(a{2,3})+`,
  `(?:a|a){0,24}`). It also covers a regex the
  compiler cannot analyse, such as a backreference inside an unbounded repeat.
- `bynk.types.polynomial_regex`: two repetitions that can split the same text
  (`\d*\d*`). With a `MaxLength` or `Length` predicate in the same refinement
  small enough for the pattern's degree (at most 3,162 characters for two
  overlapping repetitions, 215 for three) it is only a warning,
  `bynk.types.polynomial_regex_capped`, because length predicates are always
  checked before `Matches`.

`(foo|foobar)+` is accepted: its alternatives overlap, but each text splits into
them only one way.

## `.of` — checked construction

```bynk,fragment
Age.of(value)   // Result[Age, ValidationError]
```

`.of` **always** returns a `Result`. Use it for values not known at compile time
(input, variables). See
[Define a refined type and validate untrusted input](/book/guides/type-system/define-and-validate/).

> **Tip.** In VS Code, hover a refined type and follow **Try values…** to see
> example values and check your own against the predicate, with the failing
> clause named. See [editor support](/docs/editor-and-tooling/editor-support/).

## Literal admission

A literal written where a refined type is expected is checked **at compile time**
and admitted directly (lowering to an inline brand cast), with no `Result`.
Admission applies in these positions:

- return position (block tail);
- a `let` with a type annotation;
- an `Ok`/`Some`/`Err` payload;
- a refined-typed call argument.

A literal that violates the predicate is a compile error
([`bynk.refine.literal_violates`](/book/troubleshooting/refine-literal-violates/)).
**Opaque types are excluded** from admission. Admitted literals are compile-time
literals only — integers, strings, booleans, and `()` — not arithmetic
expressions or identifiers.

See [The refined-literal admission model](/book/guides/type-system/refined-literal-admission/)
for the rationale.

## Narrowing with `is`

A runtime value can be narrowed to a refined type with `is`. `value is Refined`
runs the type's predicates at runtime and yields a `Bool`; where that truth gates
the branch (an `if` body, the right of `&&`), the value is narrowed to the refined
type — so it can be passed where the refined type is expected, without going
through `.of`:

```bynk
commons demo

type Quantity = Int where InRange(1, 100)

fn double(q: Quantity) -> Int {
  2
}

fn classify(n: Int) -> Int {
  if n is Quantity {
    double(n)        // n : Quantity here
  } else {
    0
  }
}
```

- The value must be an **identifier** to be narrowed (a `let` binding or a
  parameter); `f(x) is Quantity` is a valid check but narrows nothing.
- The refined type's base must match the value's
  ([`bynk.types.is_base_mismatch`](/book/troubleshooting/is-base-mismatch/)).
- This is the flow-sensitive counterpart to `.of`: `.of(v)` returns a `Result`
  for the untrusted case; `is` narrows in a guard. Refinements are **not**
  preserved through arithmetic (`a + b` of two refined `Int`s is a plain `Int`)
  or through an inherited base method (see below): a read that leaves the refined
  type yields the base type.

See [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/).

## Inherited base methods

A refined type inherits its base type's **read-only kernel methods**. Calling one
on a refined value type-checks exactly as it does on the base, and the result is
the **base type** — never the refined type:

```bynk
commons demo

type Name = String where NonEmpty

fn shout(n: Name) -> String { n.toUpper() }        // n.toUpper() : String
fn size(n: Name)  -> Int    { n.length() }         // n.length()  : Int
fn tag(n: Name)   -> String { "user:".concat(n) }  // a refined arg widens too
```

This is the same rule refinement already follows for arithmetic, comparison, and
ordering keys: a refined value widens to its base wherever it is *read*, so no new
mental model is needed. The inherited set is each base's entire kernel — the
String, numeric (`Int`, `Float`, `Decimal`), `Duration`, `Instant`, and `Bytes` kernels defined in the
[static semantics](/book/spec/static-semantics/). `Bool` has no kernel, so a
`Bool`-based refinement inherits nothing.

Two boundaries:

- **Declared methods win.** If the refined type declares an instance method, it
  takes precedence; the inherited kernel is consulted only when the type declares
  no method of that name.
- **Opaque types do not inherit.** An `opaque` type deliberately does not widen to
  its base, so a kernel call on it stays `bynk.types.method_not_found`. Reach its
  base through the type's own methods (or `.raw`, inside the defining commons).
