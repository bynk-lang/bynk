---
title: Share code between projects
---
**Goal:** put commons and adapters in a package, and use them from another
project with a path dependency.

A **package** is a project with an identity: a `[package]` table naming its
organisation and package, and the units it shares. Another project depends on
it by directory, gives it an **alias**, and reaches its units as
`<alias>.<unit>`.

Path and git dependencies work today, and so do dependencies only your tests
use. Registry dependencies come in a later release.

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

## Depend on a git repository

A package can also come from a git repository, at a tag, a branch or a commit:

```toml
[dependencies]
ledger = { package = "acme/ledger", git = "https://git.acme.internal/ledger", rev = "v0.3.0" }
```

The first `bynk check` (or `test`, `dev`, `deploy`, or `bynk fetch`) clones it,
checks the commit out into a shared store, and writes `bynk.lock`, which pins
the commit and a checksum of its files. Your own git does the fetching, so a
private repository works through your usual ssh agent or credential helper.

**Commit `bynk.lock`.** Every later build uses the locked commit, even if the
branch moves upstream, until you run `bynk update`. Changing `rev` in
`bynk.toml` re-resolves that dependency on the next command.

**In CI**, fetch once (or restore a cached store), then build exactly what was
committed, with no network:

```bash
bynk fetch
bynk check --locked --offline
```

`--locked` fails if the lock would change; `--offline` fails if anything needs
fetching. `bynkc` never fetches: a dependency missing from the store is one
`bynk.package.not_fetched` error telling you to run `bynk fetch`.

## Dependencies only tests use

Put packages only your suites need under `[dev-dependencies]`. A suite can
reach them, even target one of their units directly, but your commons,
contexts and adapters can't, and a deployable build never includes them:

```toml
[dev-dependencies]
fx = { package = "acme/fixtures", path = "../fixtures" }
```

```bynk,fragment
suite fx.sample

case "three is three" {
  expect three() == 3
}
```

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
