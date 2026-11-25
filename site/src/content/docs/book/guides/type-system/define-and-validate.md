---
title: Define a refined type and validate untrusted input
---
**Goal:** constrain a value with a predicate, then safely admit a value that is
not known until runtime.

## Define the type

Write a base type followed by `where` and one or more predicates (combine them
with `&&`):

```bynk
commons signup {
	type Age = Int where InRange(0, 150)

	type Username = String where MinLength(3) && MaxLength(20)
}
```

For the full predicate list, see the
[refined-type reference](/reference/refined-types/).

The base may be `Int`, `Float`, or `String`. `Float` is a distinct primitive —
it never coerces to or from `Int` — and takes float bounds:

```bynk
commons pricing {
	type Ratio = Float where InRange(0.0, 1.0)

	type Price = Float where Positive
}
```

A `String` can be constrained by a pattern too. `Matches` runs the regex in
Unicode mode, so `\p{L}` admits letters beyond ASCII, which an international name needs:

```bynk
commons people {
	type DisplayName = String where MinLength(1) && MaxLength(40) && Matches(r"[\p{L}\p{M}' -]+")
}
```

(A raw string, `r"…"`, because `\p` is not an escape in an ordinary string.) `\w` and
`\d` stay ASCII in JavaScript; use `\p{L}` and `\p{Nd}` for the Unicode forms. A leading
`(?i)` makes the match case-insensitive.

## Write a bound once

A limit often matters in more than one place: the type, an error message, a
test. Declare it as a constant and name it in the predicate; the compiler
replaces the name with the literal before it reads the refinement, so the type
admits and rejects exactly what the literal form would:

```bynk
commons shop {
	const MaxQty: Int = 100

	const SlugPattern: String = "^[a-z]+$"

	type Qty = Int where InRange(1, MaxQty)

	type Slug = String where Matches(SlugPattern)
}
```

The constant is declared exactly the argument's type (`Int` for `InRange(1, MaxQty)`
and the length predicates, `String` for `Matches`) and bound to a literal or to another
such constant, and may come from a commons the unit `uses`. A computed bound such as
`MaxQty + 1` is not supported, and a constant that does not meet the rule is
`bynk.refine.bound_not_literal`. See [constants](/reference/constants/).

## Validate with `.of`

Untrusted input must be checked at runtime. Every refined type has an `.of`
constructor that returns a `Result`:

```bynk,fragment
fn parseAge(raw: Int) -> Result[Age, ValidationError] {
  Age.of(raw)
}
```

`Age.of(raw)` returns `Ok(age)` if `raw` satisfies the predicate, or
`Err(validationError)` otherwise.

## Map a failure to a form message

A `ValidationError` names the rule that failed as `e.rule`, a `ValidationRule` a `match`
can be exhaustive over, so a form can say what is wrong without parsing English:

```bynk,fragment
fn noteProblem(e: ValidationError) -> String {
  match e.rule {
    MaxLength(limit) => "Keep it to \(limit) characters",
    MinLength(limit) => "At least \(limit) characters",
    NonEmpty => "Required",
    _ => e.message,
  }
}
```

The `_ => e.message` arm is the way to opt out of the rest: `message` is still the
human default, generated from the rule. An HTTP client reading a `400` gets the same
rule in `violation.rule`, and never the rejected value.

## Handle the `Result`

Propagate the error with `?` inside a function that returns a `Result`:

```bynk,fragment
fn register(name: String) -> Result[Username, ValidationError] {
  let u = Username.of(name)?
  Ok(u)
}
```

…or branch on both cases with `match`:

```bynk,fragment
fn label(raw: Int) -> String {
  match Age.of(raw) {
    Ok(a) => "valid age"
    Err(e) => "invalid age"
  }
}
```

A `ValidationError` carries two `String` fields, `field` and `message`, so a
failure can be explained in a log line or a response:

```bynk,fragment
fn explain(raw: Int) -> String {
  match Age.of(raw) {
    Ok(a) => "valid age"
    Err(e) => "invalid age — \(e.field): \(e.message)"
  }
}
```

The error itself has no string form, so interpolating it whole is refused, and
the rejected value is not reachable from Bynk yet:

```bynk,fail
{{#include ../../../diagnostics/interpolate_validation_error.bynk}}
```

```text
{{#include ../../../diagnostics/interpolate_validation_error.txt}}
```

## Related

- To admit a value you *do* know at compile time, see
  [Use a literal where a refined type is expected](/book/guides/type-system/use-a-literal/).
- Reference: [refined-type API](/reference/refined-types/).
- Why it works this way: [The type-system philosophy](/book/concepts/type-system/philosophy/).
