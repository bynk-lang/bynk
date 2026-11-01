---
title: "`bynk.generics.missing_codable_bound`"
---
```text
[bynk.generics.missing_codable_bound] `Json.encode` encodes this value, but type parameter `T` has no `Codable` bound
```

## What it means

A generic function encodes or decodes a type parameter with `Json.encode` or
`Json.decode[T]`, or passes it to a `Codable`-bounded generic function or capability
operation, but the parameter is not declared `Codable`. Any `T` could be a
function, so the compiler needs the requirement written in the signature, where the
call site can supply the codec.

```bynk,fail=bynk.generics.missing_codable_bound
commons demo.missing.codable.bound

fn show[T](v: T) -> String { Json.encode(v) }
```

## Fix

The error carries a machine-applicable fix; in an editor, apply it, or run **Fix
all**. `bynk check --fix` applies the same fix from the command line. By hand:

- a function's own parameter: `fn show[T: Codable](v: T) -> String { … }`;
- a method's receiver type parameter: `fn Box.show(self) -> String where A: Codable { … }`;
- a parameter already declared `Eq`: add `Codable` beside it, `T: Eq + Codable`.
  Unlike `Ord` and `Key`, `Codable` does not imply `Eq`.

A call that passes a type parameter on to a `Codable`-bounded one reports the same
code at the call; declare the bound on the caller.

## If the error is `bynk.types.not_codable` instead

The bound is fine, but the type you used at the call cannot cross the boundary: it is
`()`, a function, an `Effect`, a stream, or another type `Json.decode` refuses.

## Related

- Reference: [Generics and bounds](/book/reference/generics/#codable)
- Guide: [Generic code that crosses the boundary](/book/guides/type-system/generic-code-that-crosses-the-boundary/)
