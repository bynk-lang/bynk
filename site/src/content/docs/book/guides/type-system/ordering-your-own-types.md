---
title: Understand ordering your own types
---
**Read this when** you want to sort your own records or enums, compare two of
them with `<`, or write a generic function that needs an order.

## Why ordering is opt-in

A type has an order only if someone decides what it is. For `Priority`, is `High`
above `Low`, or the reverse? For a `Task`, does `due` or `title` come first? The
compiler will not guess, so a record, sum, enum or opaque type you declare is not
orderable until you say so:

```bynk
commons guide.priorities

type Priority = enum { Low, Medium, High } derives Ord

type Task = { priority: Priority, due: Instant, title: String } derives Ord

fn moreUrgent(a: Task, b: Task) -> Bool { a > b }
```

`derives Ord` goes after the type body. It requires every field and payload to be
orderable already (`Int`, `Float`, `String`, `Duration`, `Instant`, `Bytes`,
`Option` or `List` of an orderable type, another `derives Ord` type, and so on),
and names the field if one is not (`bynk.types.derives_field_not_orderable`).
`Bool`, `Result`, `Map`, `Set` and functions are not orderable.

## Declaration order is the order

The order comes straight from how you wrote the declaration:

- a **record** compares its first field, and only if those are equal, its second,
  and so on. `Task` above sorts by `priority`, then `due`, then `title`;
- a **sum or enum** compares by variant position, so `Low < Medium < High`; two
  values of the same variant compare by payload.

That makes declaration order part of the type's meaning. Moving `due` above
`priority`, or `High` above `Low`, changes how values sort, and nothing warns you:
the wire contract ignores field and variant order. If a `derives Ord` type is
sorted anywhere you care about, treat reordering its declaration as a behaviour
change.

For the same reason the compiler will not add `derives Ord` for you. When you
compare a type that lacks it (`bynk.types.not_orderable`), the editor offers the
fix, but **Fix all** and `bynk check --fix` leave it alone.

## Sort, min and max

With `uses bynk.list`, `sort`, `min` and `max` work on a list of any orderable
type:

```bynk,fragment
commons guide.sorting

uses bynk.list

type Priority = enum { Low, Medium, High } derives Ord

type Task = { priority: Priority, title: String } derives Ord

fn ranked(ts: List[Task]) -> List[Task] { sort(ts) }

fn hottest(ts: List[Task]) -> Option[Task] { max(ts) }

fn byTitle(ts: List[Task]) -> List[Task] { ts.sortBy((t) => t.title) }
```

`min` and `max` return an `Option`, `None` for an empty list. The `sortBy`, `min`
and `max` methods take any orderable key too, so a key can be a whole `Task` or
an `Option[Int]`.

## Generic code: `T: Ord`

A generic function that orders its parameter says so, the same way `Eq` is
declared:

```bynk,fragment
commons guide.generic

uses bynk.list

fn top[T: Ord](xs: List[T]) -> Option[T] { max(xs) }

fn clamp[T: Ord](x: T, lo: T, hi: T) -> T {
  if x < lo { lo } else if x > hi { hi } else { x }
}
```

`T: Ord` also allows `==`, so you do not need `T: Eq + Ord` (the compiler warns
about it). Callers must pass an orderable type; `top` over a record without
`derives Ord` is rejected at the call. If you forget the bound, the compiler
points at the comparison and offers the fix: see
[`bynk.generics.missing_ord_bound`](/book/troubleshooting/generics-missing-ord-bound/).

## Three edge cases

**`Float` and `NaN`.** The sort functions treat `NaN` as larger than every other
number, equal to itself, and `-0` as equal to `0`, so a list containing `NaN`
sorts predictably, with `NaN` last. The operators are different: `NaN < x` and
`NaN > x` are both `false`, as in IEEE arithmetic, and `==` is unchanged.

**`Bytes`.** `Bytes` is orderable: octet by octet as unsigned values, a shorter
prefix first. It was equatable but not orderable before; code that was rejected
for sorting on a `Bytes` key now compiles.

**`String`.** Strings compare by UTF-16 code unit, which matches the order
JavaScript's `<` gives. It is neither Unicode code-point nor locale order, so
`"Z"` sorts before `"a"`.

Reference: [Ordering your own types](/book/reference/types/#ordering-your-own-types)
and [Generics and bounds](/book/reference/generics/).
