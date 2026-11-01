---
title: "`bynk.generics.missing_key_bound`"
---
```text
[bynk.generics.missing_key_bound] a `Map` key must be `Key`, but type parameter `K` has no `Key` bound
```

## What it means

A generic function or type uses a type parameter as a `Map` key, a `Set` member,
or the key of `distinct`, `distinctBy`, `groupBy` or a join, or passes the
parameter to a `Key`-bounded generic, but the parameter is not declared `Key`.
Any `K` could be a function or a `Float`, so the compiler needs the requirement
written in the signature.

```bynk,fail=bynk.generics.missing_key_bound
commons demo.missing.key.bound

fn single[K](k: K) -> Map[K, Int] {
  let m: Map[K, Int] = Map.empty()
  m.insert(k, 1)
}
```

## Fix

The error carries a machine-applicable fix; in an editor, apply it, or run **Fix
all**. `bynk check --fix` applies the same fix from the command line, which is how
a project written before `Key` is migrated. By hand:

- a function's own parameter: `fn single[K: Key](k: K) -> Map[K, Int] { … }`;
- a generic type's parameter, on the type: `type Index[K: Key, V] = { m: Map[K, V] }`;
- a method's receiver type parameter: `fn Box.keyed(self, s: Set[A]) -> Int where
  A: Key { … }`;
- a parameter already declared `Eq`: replace `Eq` with `Key`. `Key` implies `Eq`,
  and keeping both is the warning `bynk.generics.redundant_eq_bound`.

A call that passes a type parameter on to a `Key`-bounded one reports the same
code at the call; declare the bound on the caller.

## If the error is `bynk.types.not_keyable` instead

The bound is fine, but the type you used cannot be a key: it contains a `Float`
(`NaN` is not equal to itself), a `Map`, a `Set`, or a function. Key by a field
that is a `Key`, or by a `String` made from the value.

## Related

- Reference: [Generics and bounds](/book/reference/generics/#key) and
  [Keys](/book/reference/types/#keys)
- Guide: [Key a map by a record](/book/guides/type-system/key-a-map-by-a-record/)
