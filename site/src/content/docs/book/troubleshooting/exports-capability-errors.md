---
title: "`bynk.exports.*` / `bynk.given.cross_context_*` errors"
---
These diagnostics relate to **sharing a capability across contexts** —
`exports capability` and a qualified `given B.Cap`. See
[Share a capability across contexts](/book/guides/effects-and-capabilities/share-across-contexts/).

## `bynk.exports.undeclared_capability`

```text
[bynk.exports.undeclared_capability] `exports capability` references `Nope`, which is not a capability declared in context `platform.time`
```

`exports capability { … }` may only name capabilities the context **declares**.
Declare the capability (or fix the name):

```bynk,fragment
exports capability { Clock }   // not `{ Nope }`

capability Clock { fn now() -> Effect[Int] }
```

Type exports (`exports opaque` / `exports transparent`) and capability exports
are separate name kinds — a type cannot appear in `exports capability`.

## `bynk.exports.capability_not_provided`

```text
[bynk.exports.capability_not_provided] exported capability `Clock` has no provider in context `platform.np` — a consumer cannot instantiate it
```

An exported capability must also be **provided** in the same context, so a
consumer's composition can instantiate it. Add a provider:

```bynk,fragment
provides Clock = SystemClock {
  fn now() -> Effect[Int] { 0 }
}
```

## `bynk.given.cross_context_unknown_capability`

```text
[bynk.given.cross_context_unknown_capability] context `platform.clk` does not export a capability named `Clock`
```

`given B.Cap` requires `B` to **export** `Cap`. Add the capability to `B`'s
`exports capability { … }` clause. (If `B` is not in this context's `consumes`
clauses at all, you'll see `bynk.resolve.unconsumed_context` instead — add the
`consumes` clause first.)

## Related

- [Share a capability across contexts](/book/guides/effects-and-capabilities/share-across-contexts/)
- Reference: [Capabilities & providers](/book/reference/capabilities/)
