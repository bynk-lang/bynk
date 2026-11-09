---
title: "`bynk.val.*` errors"
---
`Val[T]` fabricates values **in tests only**. These are its common errors, along
with the `bynk.property.*` diagnostics for the generative `property` / `for all`
form.

## `bynk.val.outside_test`

```text
[bynk.val.outside_test] Error: `Val[T]` is only valid inside a test case body
```

**Cause:** you used `Val[T]` outside a `case "…" { … }` — for example in a
regular function.

**Fix:** move the `Val[T]` into a test case. To construct a value in production
code, use a real constructor instead (`.of` for a refined or opaque type — or
`.unsafe` for an opaque type within its defining commons; a record/variant literal
otherwise).

## `bynk.val.needs_pin`

```text
[bynk.val.needs_pin] bare `Val[Code]` cannot generate a value for a `Matches` refinement
```

**Cause:** you wrote a bare `Val[T]` (or used the type in a `for all`) for a type
whose refinement is a `Matches` pattern. Bynk cannot invent a string that matches
an arbitrary regex, so the type has no generator.

**Fix:** pin a concrete value that satisfies the pattern. Given the type:

```bynk
type Code = String where Matches("[a-z]+")
```

…pin the value where you fabricate it in a test case:

```bynk,fragment
let c = Val[Code]("abc")
```

or give the type an example, which every bare `Val[Code]` then uses (the
compiler checks it matches the pattern):

```bynk,fragment
type Code = String where Matches("[a-z]+") @example("abc")
```

A `for all` over the type is still refused: an example is one value, so a
property over it would run one case. See
[`@example`](/book/reference/testing/#val-example).

## `bynk.val.agent_not_generable`

```text
[bynk.val.agent_not_generable] Error: an agent cannot be fabricated with `Val`
```

**Cause:** you named an agent type in a `Val[T]` or a `for all` binding. An agent
has no refinement domain to draw an inhabitant from.

**Fix:** address the agent by constructing it with a key (`Link(code)`) and call
its handlers instead. Behavioural agent testing over handler sequences is a later
slice.

## `bynk.property.restates_refinement`

```text
[bynk.property.restates_refinement] Warning: this property only re-checks a refinement `Quantity` already guarantees
```

**Cause:** a `property` whose `expect` merely re-asserts a refinement the generated
type already enforces — for example `for all q: Quantity { expect q > 0 }` when
`Quantity` is `Int where Positive`. Every generated `q` is already positive, so the
property proves nothing.

**Fix:** delete the property, or make it assert a claim about your *code* rather
than about the type's domain (a relationship between inputs and an output). This is
a conservative, syntactic check — it fires only on the obvious restatements.

## `bynk.property.where_not_bool`

```text
[bynk.property.where_not_bool] Error: a `where` filter must be `Bool`
```

**Cause:** the `where <pred>` on a `for all` is not a `Bool` — the same fail-closed
check `expect`, `invariant`, and `ensures` get.

**Fix:** make the filter a pure `Bool` predicate (`a <= b`, `is`, `implies`, the
operators, pure methods).

## Other Val errors

- The type doesn't resolve, or its kind can't be generated — check the type name
  and see the [testing reference](/book/reference/testing/)
  (`bynk.val.unknown_type`, `bynk.val.unsupported_kind`).
- A pin that isn't a compile-time literal is rejected (`bynk.val.pin_not_literal`),
  as is a pin on a kind that does not support one (`bynk.val.pin_unsupported`), or
  the wrong number of pin arguments (`bynk.val.arity`).
- A pinned value that violates the refinement is rejected
  (`bynk.val.literal_violates`) for the same reason a literal would be (see
  [`bynk.refine.literal_violates`](/book/troubleshooting/refine-literal-violates/)).

## Related

- [Write tests and stub collaborators](/book/guides/testing/write-tests/)
- Reference: [testing](/book/reference/testing/)
