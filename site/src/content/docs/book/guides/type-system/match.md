---
title: "Pattern-match with `match`"
---
**Goal:** branch on the variants of a sum type (or a `Result`/`Option`), binding
each variant's payload.

## Match on a sum type

`match` requires an arm for **every** variant. Name a variant to match it; bind
its payload by naming the fields:

```bynk
commons shop {
  type Status =
    | Pending
    | Shipped(tracking: String)
    | Cancelled(reason: String)

  fn describe(s: Status) -> String {
    match s {
      Pending => "awaiting shipment"
      Shipped(tracking: t) => t
      Cancelled(reason: r) => r
    }
  }
}
```

Omit a variant and the program does not compile — there is no accidental
fall-through. A `match` is an expression: its value is the value of the matched
arm.

## Match on `Result` and `Option`

The same form works for the built-in sum types:

```bynk
fn label(o: Option[Int]) -> String {
  match o {
    Some(n) => "present"
    None => "absent"
  }
}
```

## Match on a primitive value

`match` also dispatches a primitive `Int`, `String`, or `Bool` against literal
patterns — the idiomatic way to map a raw value (an external reference code, a
flag) to a domain type, instead of an `if`/`else if ==` chain:

```bynk
fn classify(code: Int) -> String {
  match code {
    31 => "english"
    32 => "irish"
    _  => "other"
  }
}
```

`Int` and `String` are unbounded, so a literal `match` needs a wildcard `_` arm
to be exhaustive. `Bool` is complete once both `true` and `false` appear:

```bynk
fn label(flag: Bool) -> String {
  match flag {
    true  => "on"
    false => "off"
  }
}
```

Literal patterns match by value — an integer (which may be negative, `-1 => …`),
a string, or a boolean. They are `match`-only: to test a single value elsewhere,
use `==`. A refined type (`type Code = Int where …`) is matched against the same
literals as its base.

## Guard a primitive pattern with `where`

`_ where predicate` dispatches on a *range* or *shape* instead of an exact
value, reusing the same closed predicate vocabulary a refined type declares
with (`InRange`, `Matches`, `NonEmpty`, …):

```bynk
fn classify(status: Int) -> String {
  match status {
    _ where InRange(200, 299) => "success"
    _ where InRange(400, 499) => "client error"
    _ where InRange(500, 599) => "server error"
    _                         => "other"
  }
}
```

A refined pattern is a **guard, not a narrowing**: it does not change the
static type of anything in the arm's body, and — like an `if` guard — an arm
alone never satisfies exhaustiveness, since its predicate might fail at
runtime. A refined-only arm set still needs a wildcard `_` arm. The inner form
is always `_` — `31 where InRange(0, 10)` and other non-wildcard inners are
rejected, since matching a specific value is already what a plain literal
pattern does. Refined patterns are `match`-only, the same restriction as
literal patterns: `is` already has its own refinement check over a *named*
refined type ([Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/)).

## Discriminate a nested payload

A payload binding is itself a pattern, so you can look *inside* a variant's
payload instead of only naming it. This is how you discriminate an error cause,
or unwrap an `Option` of a `Result` in one flat `match` — no second, nested
`match`:

```bynk
commons monitor {
  type FetchError = enum { PollClosed, UnknownChoice }

  fn code(res: Result[Int, FetchError]) -> Int {
    match res {
      Ok(n)              => n
      Err(PollClosed)    => 0
      Err(UnknownChoice) => 400
    }
  }

  fn inner(opt: Option[Result[Int, FetchError]]) -> Int {
    match opt {
      Some(Ok(n))  => n
      Some(Err(_)) => -1
      None         => -2
    }
  }
}
```

Exhaustiveness sees through the nesting: `Some(Ok(_))` / `Some(Err(_))` / `None`
covers every case **without** a wildcard, and omitting `Err(UnknownChoice)`
above is a compile error naming the uncovered shape. Capitalisation
disambiguates a **binding** from a **variant**: a lowercase name (`n`) binds the
payload; an uppercase name (`PollClosed`) matches that nested variant.

## Guard an arm with `if`

An arm may carry a trailing `if` **guard** — an arbitrary `Bool` expression over
the arm's bindings. The arm matches only when the pattern matches *and* the guard
holds:

```bynk
commons routing {
  type Req =
    | Get(path: String)
    | Post(body: String)

  fn route(r: Req) -> String {
    match r {
      Get(path) if path == "/api" => "api"
      Get(path)                   => path
      Post(body)                  => body
    }
  }
}
```

A guarded arm never counts toward exhaustiveness — the guard can fail at runtime
— so the following unguarded `Get(path)` arm stays reachable. `if` takes an
arbitrary `Bool` expression; for a guard drawn from the closed refinement
vocabulary over a primitive scrutinee, see [Guard a primitive pattern with
`where`](#guard-a-primitive-pattern-with-where) above — the two compose (a
pattern can carry both a `where` and a trailing `if`).

## Match several patterns with `|`

An **or-pattern** `p₁ | p₂` matches if either alternative matches — useful when
several variants (or literals) share a body:

```bynk
fn small(n: Int) -> String {
  match n {
    1 | 2 | 3 => "small"
    _         => "large"
  }
}
```

Every alternative must bind the **same set of names**, and a name shared across
alternatives must have the **same type** in each — this is what lets the arm's
body (and an optional trailing guard) see one consistent set of bindings
regardless of which alternative matched:

```bynk
commons booking {
  type State =
    | Held(guest: String, room: Int, days: Int, rsv: Int)
    | Confirmed(who: String, room: Int, days: Int, rsv: Int)
    | Cancelled(reason: String)

  fn roomOf(s: State) -> Int {
    match s {
      Held(_, r, _, rsv) | Confirmed(_, r, _, rsv) if rsv > 0 => r
      Held(_, r, _, _)   | Confirmed(_, r, _, _)              => r
      Cancelled(_)                                            => 0
    }
  }
}
```

An or-pattern covers all its alternatives for exhaustiveness, and composes with
`is` (see [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/)):
`if state is (Held(_, r, ...) | Confirmed(_, r, ...)) { … r … }` narrows and
binds `r` in the truthy branch.

## Related

- For a one-branch test that yields a `Bool`, see
  [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/).
- Reference: [type system](/book/reference/types/).

To match a record by its fields, see [Match on a record's fields](/book/guides/type-system/match-on-a-records-fields/).
