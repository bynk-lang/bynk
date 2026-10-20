---
title: Share code between projects
---
**Goal:** put commons and adapters in a package, and use them from another
project with a path dependency.

A **package** is a project with an identity: a `[package]` table naming its
organisation and package, and the units it shares. Another project depends on
it by directory, gives it an **alias**, and reaches its units as
`<alias>.<unit>`.

Path dependencies work today. Git and registry dependencies, and dependencies
only tests use, come in later releases.

## Lay out a workspace

Packages and the projects that use them usually live in one
[workspace](/book/guides/projects-build-and-deployment/layout/#workspaces):

```text
shop/
├── bynk.toml               # [workspace] members = ["packages/*", "apps/*"]
├── packages/
│   └── money/
│       ├── bynk.toml       # [package] name = "acme/money"
│       └── src/
│           ├── currency.bynk
│           └── rounding.bynk
└── apps/
    └── shop/
        ├── bynk.toml       # [dependencies] money = { … }
        └── src/
            └── pricing.bynk
```

## Make a package

Give the project a `[package]` table instead of `[project]`, and list the
units it shares in `exports`:

```toml
# packages/money/bynk.toml
[package]
name = "acme/money"
version = "1.0.0"
exports = ["currency"]
```

Write its sources exactly as in any project. Inside the package, its units use
each other by their own names:

```bynk,fragment
commons currency

uses rounding

fn double(c: Int) -> Int {
  half(c) * 4
}
```

A package shares commons and adapters, never a context: a context is what a
project deploys. Leave `exports` out to share every commons and adapter.

## Depend on it

In the project that uses it, add the package under `[dependencies]`. The key is
the alias:

```toml
# apps/shop/bynk.toml
[project]
name = "shop"

[dependencies]
money = { package = "acme/money", path = "../../packages/money" }
```

Then reach its units through the alias:

```bynk,fragment
commons pricing

uses money.currency

fn total(n: Int) -> Int {
  double(n)
}
```

A dependency adapter works the same way. A context consumes it by its aliased
name, and names its capabilities through that name:

```bynk,fragment
context auth.sessions

consumes money.tokens

service login {
  on call(secret: String) -> Effect[String] given money.tokens.Jwt {
    let token <- money.tokens.Jwt.sign("u1", secret)
    token
  }
}
```

`bynk check`, `bynk test`, `bynk dev` and `bynk deploy` all build the
dependency in. In the editor, completion after `uses` and `consumes` offers a
dependency's exported units as `<alias>.<unit>`, hover shows which package a
unit comes from, and go-to-definition opens the dependency's source. Its units are emitted under `out/deps/acme/money/`, and only
the ones your project actually reaches. Adding a dependency you don't use
changes nothing that's emitted.

## What the checks catch

- Reaching a unit the package doesn't list in `exports`
  (`bynk.package.unit_not_exported`), or one it doesn't declare
  (`bynk.package.unknown_unit`).
- An alias that is also the first segment of one of your own units
  (`bynk.package.alias_shadows_unit`): rename the alias.
- A `package` that isn't the directory's `[package] name`
  (`bynk.package.name_mismatch`).
- One package reached from two directories
  (`bynk.package.duplicate_source`), or a cycle
  (`bynk.package.dependency_cycle`).
- A dependency's own path dependency outside the workspace
  (`bynk.dependency.path_outside_root`). Your own project's path entries can
  point anywhere; a dependency's can only reach other workspace members.

The same package reached through two aliases (yours, and another
dependency's) is one package, with one set of types.

**Not yet:** a dependency can't carry an adapter with npm `requires`, or an
`asset`.

## Related

- [Reference — `[dependencies]`](/docs/manifest/#dependencies) and
  [`[package]`](/docs/manifest/#package).
- [Lay out a project — Workspaces](/book/guides/projects-build-and-deployment/layout/#workspaces).
