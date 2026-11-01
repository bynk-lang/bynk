---
title: Declare `Eq` on a generic that compares
---
**Do this when** a generic function or method compares values of its type
parameter with `==` or `!=`, or passes the parameter to another generic that does.
The compiler says exactly where, and offers the fix.

## A function

Add `: Eq` to the parameter:

```bynk
commons guide.same

fn same[T: Eq](a: T, b: T) -> Bool { a == b }

fn twice[U: Eq](a: U, b: U) -> Bool { same(a, b) }
```

`twice` never writes `==` itself, but it hands `U` to `same`, whose `T` is `Eq`,
so `U` must be declared `Eq` as well.

## A method on a generic type

A method compares the receiver type's parameter, which it does not declare. Bound
it with a `where` clause after the return type:

```bynk
commons guide.boxes

type Box[A] = { value: A }

fn Box.has(self, x: A) -> Bool where A: Eq { self.value == x }
```

## Let the editor do it

`bynk.generics.missing_eq_bound` carries a machine-applicable fix. In an editor
it is a quick fix, and **Fix all** (`source.fixAll.bynk`) applies every one in
the file in a single action. If a chain of generics needs bounds, run it again
until nothing is left: each round exposes the next link.

## What a bound changes for callers

A call is checked against the declared bound. `same(f, h)` on two functions fails
at the call, because functions are not equality-supporting, and the signature
says so up front (hover on `same` shows `[T: Eq]`). A generic that never compares
its parameter stays unbounded and keeps accepting functions.

Reference: [Generics and bounds](/book/reference/generics/). If you are upgrading
older code, see
[`bynk.generics.missing_eq_bound`](/book/troubleshooting/generics-missing-eq-bound/).
