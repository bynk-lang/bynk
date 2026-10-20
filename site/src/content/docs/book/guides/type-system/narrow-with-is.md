---
title: "Narrow and bind with `is`"
---
**Goal:** test whether a value matches a variant — and optionally bind its
payload — as a `Bool` expression.

An `is` expression tests a value against a variant pattern and evaluates to a
`Bool`. Use it where you want a single condition rather than a full `match`.

## Test a variant

```bynk
commons test {
  fn isOk(r: Result[Int, String]) -> Bool {
    r is Ok(_)
  }
}
```

The wildcard `_` ignores the payload. This compiles to a tag check
(`r.tag === "Ok"`).

## Bind a payload in an `if`

When the pattern names a variable, that variable is in scope in the `if`'s
then-branch:

```bynk
commons test {
  fn useValue(r: Result[Int, String]) -> Int {
    if r is Ok(n) {
      n
    } else {
      0
    }
  }
}
```

Bindings also reach the right operand of `&&` (and of `implies`), which runs
only when the left matched, and the `else` of a negated test:

```bynk
commons test {
  fn positive(o: Option[Int]) -> Bool {
    o is Some(v) && v > 0
  }

  fn orZero(o: Option[Int]) -> Int {
    if !(o is Some(v)) { 0 } else { v }
  }
}
```

They don't reach the right operand of `||`, which runs when the left *didn't*
match, or anything after the `if`.

## Narrow to a refined type

`is` also works on **refined types**: `value is Quantity` runs the refinement's
predicates at runtime and, in the positive branch, narrows the value to the
refined type — so you can pass it where that type is expected without `.of`:

```bynk
commons demo

type Quantity = Int where InRange(1, 100)

fn double(q: Quantity) -> Int {
  2
}

fn classify(n: Int) -> Int {
  if n is Quantity {
    double(n)        // n : Quantity
  } else {
    0
  }
}
```

The value must be an identifier to be narrowed, and the refined type's base must
match it ([`bynk.types.is_base_mismatch`](/book/troubleshooting/is-base-mismatch/)).
Use `.of` instead when you need to handle the failure as a value.

## Test several patterns with `|`

The pattern after `is` can be an or-pattern — parentheses are recommended for
readability but not required:

```bynk
commons booking {
  type State =
    | Held(guest: String, room: Int)
    | Confirmed(who: String, room: Int)
    | Cancelled(reason: String)

  fn roomOf(s: State) -> String {
    if s is (Held(_, r) | Confirmed(_, r)) {
      "room \(r)"
    } else {
      "no room"
    }
  }
}
```

`r` is bound in the then-branch regardless of which alternative matched — every
alternative of an or-pattern must bind the same names at the same type, exactly
as in a [`match` arm](/book/guides/type-system/match/).

## In assertions

Because `is` yields a `Bool`, it pairs naturally with `expect` in tests:

```bynk,fragment
expect result is Ok(_)
```

The receiver can be any expression of a sum/`Result`/`Option` type — an
identifier, a field access, or a call result.

## Related

- For exhaustive multi-way branching, use [`match`](/book/guides/type-system/match/).
- To bind a payload for the rest of a block, and say what happens when it is
  absent, use [`let … else`](/book/guides/type-system/result-and-optionals/#let-else).
- [Write tests and stub collaborators](/book/guides/testing/write-tests/).
