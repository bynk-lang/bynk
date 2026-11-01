---
title: "`bynk.generics.missing_eq_bound`"
---
```text
[bynk.generics.missing_eq_bound] `==` compares values of this type, but type parameter `T` has no `Eq` bound
```

## What it means

A generic function or method compares a value whose type mentions a type
parameter, or passes the parameter to a generic that does, but the parameter is
not declared `Eq`. Before constrained generics slice 0 the compiler *inferred*
this requirement; it is now written in the signature, the way every other
constraint will be.

```bynk,fail=bynk.generics.missing_eq_bound
commons demo.missing

fn same[T](a: T, b: T) -> Bool { a == b }
```

## Fix

The error carries a machine-applicable fix; in an editor, apply it, or run **Fix
all**. By hand:

- a function's own parameter: `fn same[T: Eq](a: T, b: T) -> Bool { … }`;
- a method's receiver type parameter: `fn Box.has(self, x: A) -> Bool where A: Eq { … }`;
- a parameter already bounded: add to the bound with `+`.

A call that passes a type parameter on to an `Eq`-bounded one reports the same
code at the call; declare the bound on the caller.

## Migrating older code

This is a **breaking change** for generics written before it. Every error has the
fix above, so migrating a project is applying the quick fixes (**Fix all** adds
every bound in a file, following chains of generics that call one another; run it
again if a method compares two receiver type parameters) and rebuilding. Emitted
TypeScript is unchanged. Without an editor, run `bynk check --fix`
([#2642](https://github.com/bynk-lang/bynk/issues/2642)), which applies the same
machine-applicable fixes and repeats until none is left.

## Related

- Reference: [Generics and bounds](/book/reference/generics/)
- Guide: [Declare `Eq` on a generic that compares](/book/guides/type-system/declare-eq-bounds/)
