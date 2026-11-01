---
title: "`bynk.generics.missing_ord_bound`"
---
```text
[bynk.generics.missing_ord_bound] operator `>` orders values of this type, but type parameter `T` has no `Ord` bound
```

## What it means

A generic function or method orders a value whose type mentions a type parameter
(with `<` `<=` `>` `>=`, or as a `sortBy`/`min`/`max` key), or passes the
parameter to a generic that does, but the parameter is not declared `Ord`. Any
`T` could be a function or a `Map`, so the compiler needs the requirement written
in the signature.

```bynk,fail=bynk.generics.missing_ord_bound
commons demo.missing.ord.bound

fn bigger[T](a: T, b: T) -> Bool { a > b }
```

## Fix

The error carries a machine-applicable fix; in an editor, apply it, or run **Fix
all**. `bynk check --fix` applies the same fix from the command line. By hand:

- a function's own parameter: `fn bigger[T: Ord](a: T, b: T) -> Bool { … }`;
- a method's receiver type parameter: `fn Box.biggest(self, x: A) -> Bool where
  A: Ord { … }`;
- a parameter already declared `Eq`: replace `Eq` with `Ord`. `Ord` implies `Eq`,
  and keeping both is the warning `bynk.generics.redundant_eq_bound`.

A call that passes a type parameter on to an `Ord`-bounded one reports the same
code at the call; declare the bound on the caller.

## If the error is `bynk.types.not_orderable` instead

The bound is fine, but the type you passed has no order. Either use an orderable
type, or, for a record, sum, enum or opaque type of your own, add `derives Ord`
to its declaration. That fix is offered but never applied automatically, because
it makes the declaration order of the type decide how its values sort.

## Related

- Reference: [Generics and bounds](/book/reference/generics/) and
  [Ordering your own types](/book/reference/types/#ordering-your-own-types)
- Guide: [Understand ordering your own types](/book/guides/type-system/ordering-your-own-types/)
