---
title: "`bynk.adapter.*` / `bynk.namespace.*` / `bynk.requires.*` errors"
---
These diagnostics relate to **adapters** — the host boundary. See the
[Adapters reference](/book/reference/adapters/) and
[Wrap a library as an adapter](/book/guides/effects-and-capabilities/wrap-a-library/).

## `bynk.adapter.provider_has_body`

```text
[bynk.adapter.provider_has_body] a provider inside an `adapter` must be external (no body) — its implementation is supplied by the binding
```

Providers inside an adapter are **external**: write `provides Cap = Name` with no
brace block. The implementation lives in the binding, not in Bynk.

```bynk,fragment
provides Jwt = JoseJwt        // not `provides Jwt = JoseJwt { fn sign(…) { … } }`
```

## `bynk.context.external_provider`

```text
[bynk.context.external_provider] an external (bodiless) provider is only allowed inside an `adapter` — a context provider must have a Bynk body
```

A bodiless `provides` is only legal inside an adapter. In a context, give the
provider a Bynk body — or move it to an adapter if it wraps host code.

## `bynk.adapter.disallowed_item`

```text
[bynk.adapter.disallowed_item] an `adapter` may not declare a `service`
```

Adapters contain only capabilities, boundary types, external providers, inline
pure helpers, and `exports`. Put services and agents in a `context`.

## `bynk.adapter.no_binding`

```text
[bynk.adapter.no_binding] adapter `tokens` declares an external provider but has no `binding` clause to supply its implementation
```

An adapter with external providers must name a `binding` module:

```bynk,fragment
binding "./tokens.binding.ts"
```

The same code reports when the named module cannot be read — check the path
(resolved relative to the adapter's source file) and author the `.binding.ts`.

## `bynk.requires.unpinned_dependency`

```text
[bynk.requires.unpinned_dependency] dependency `jose` has an unpinned version range `*` — pin a concrete range (e.g. `^1.2.0`)
```

Pin every dependency range. Unpinned ranges (`*`, `latest`, or anything with no
version number) make builds irreproducible and are rejected.

## `bynk.requires.conflicting_range`

```text
[bynk.requires.conflicting_range] adapter `tokens2` requires `jose` at `^4`, but adapter `tokens` requires it at `^5`
```

The project installs one version of each npm package, so every adapter that
requires a package must state the same range. Identical ranges merge silently;
different ranges are refused rather than intersected. Ranges are compared as
written, so equivalent spellings such as `^5` and `^5.0.0` still conflict. Align
the ranges, or update the binding written against the older version. The same
code reports one adapter that lists a package twice at different ranges.

## `bynk.namespace.reserved`

```text
[bynk.namespace.reserved] `bynk.time` uses the reserved `bynk` namespace — the `bynk` root is reserved for the toolchain's conformance surface
```

The `bynk` root namespace names the toolchain's conformance surface. Rename any
user `commons`/`context`/`adapter`/`test` so its first segment is not `bynk`.

## `bynk.consumes.capability_name_clash`

```text
[bynk.consumes.capability_name_clash] flattened capability `Jwt` clashes with a capability declared locally — use qualified `given tokens.Jwt` instead
```

A `consumes U { Cap }` flattened name collided with a local capability or another
flattened name. Resolve it with a qualified `given U.Cap`, or alias the unit with
`consumes U as Alias`.
