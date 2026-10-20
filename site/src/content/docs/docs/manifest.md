---
title: "`bynk.toml` manifest"
---
A `bynk.toml` at a project's root marks a directory as a Bynk project and
configures its layout and tooling. A multi-file project uses one; a lone `.bynk`
file needs none (see [Legacy mode](#legacy-mode)).

Every section and every key is optional. An omitted section falls back to its
defaults, and a manifest that lists only `[project]` behaves identically to one
that omits it. `[paths]` is parsed by the compiler; `[fmt]` by the formatter
(`bynk-fmt`), which both the CLI and the language server read it through;
`[lsp]` by the language server; `[workers]` by the compiler, for a Workers
build; `[test]` by the test runner, for a run against a deployment.

A project is an **application** (`[project]`) or a shareable **package**
(`[package]`), never both. A directory whose `bynk.toml` holds `[workspace]`
isn't a project at all: it is a [workspace root](#workspace) that groups
projects. `[package]`, `[dependencies]`, `[dev-dependencies]` and
`[registries]` are the first part of [packaging](/book/about/versioning-and-roadmap/):
they are checked today and do more as packaging lands, as each section below
says.

## Example

A fully-populated manifest, with every key set to its default:

```toml
[project]
name = "my-project"     # display only; no default
version = "0.1.0"       # display only; no default

[paths]
include = ["src", "tests"]  # trees to compile; default: the conventional
                            # roots that exist, else the project root
exclude = []                # subtrees to skip during discovery

[fmt]
indent = "tab"
indent_width = 2        # only consulted when indent = "spaces"
max_line_width = 100
trailing_comma = true

[lsp]
diagnostics_mode = "live"
diagnostics_debounce_ms = 300

[workers]
compatibility_flags = []    # extra Cloudflare flags, after the default

[unstable]
allow = []                  # experimental standard-library items to use

[test.targets.preview]      # no default: a deployment `bynk test --against` may use
url.shop-orders = "https://shop-orders-preview.example.workers.dev"
```

Delete any line you are happy to leave at its default — the file above is
equivalent to an empty `bynk.toml`, which is equivalent to `[project]` alone.

A package names itself with `[package]` in place of `[project]`, and may
declare what it depends on. Dependencies are checked but not resolved yet, so
a manifest that declares any is refused until resolution ships:

```toml
[package]
name = "acme/money"         # organisation/package; required
version = "1.4.0"           # semver; no default
description = "Money and currency commons"
license = "MIT"
repository = "https://github.com/acme/money"
exports = ["currency", "money"]   # default: every commons and adapter

[dependencies]
local = { package = "acme/local", path = "../local" }        # resolved
ledger_kit = { package = "acme/ledger_kit", git = "https://git.acme.internal/ledger-kit", rev = "v0.3.0" }  # slice 2b
fx = { package = "globex/money", version = "^2" }            # slice 4

[dev-dependencies]          # not resolved yet: refused when non-empty (slice 2b)
fixtures = { package = "acme/fixtures", path = "../fixtures" }
```

A workspace root holds only `[workspace]` and, optionally, `[registries]`:

```toml
[workspace]
members = ["packages/*", "apps/shop"]
exclude = ["packages/scratch"]

[registries.acme-internal]
index = "https://bynk.acme.internal/index"
organisations = ["acme"]
key = "ed25519:…"           # optional: the registry's pinned signing key

[registries.corp-mirror]
index = "https://mirror.example.internal/bynk"
mirror-of = "bynk"          # fetch first-party packages through here
```

## Unknown tables and keys

`bynk.toml` holds the tables below, and nothing else. An unknown table, a
key outside any table (including a table's own name given a plain value, such
as `paths = "src"`), or an unknown key in any table but `[fmt]` is an error.
In `[dependencies]`, `[dev-dependencies]` and `[registries]` the keys are
names you choose, and each entry's own keys are checked the same way. `bynkc` and `bynk` report it, with the nearest name
when there is one, and build nothing. An unknown `[fmt]` key is reported by the
formatter (`bynkc fmt`, `bynk fmt`), which owns that table, not by `check` or
`compile`:

```text
bynkc: `bynk.toml` has no table named `[pahts]` — did you mean `[paths]`?
bynkc: `[project]` has no key named `nmae` — did you mean `name`?
bynkc: `[dependencies]` `fx` has no key named `verison` — did you mean `version`?
```

A table for a feature that is planned but not built (`[deploy]`) gets the same
error, naming the issue that tracks the feature, so it can't read as working. The language server doesn't refuse
a manifest: it reads the `[paths]`, `[fmt]` and `[lsp]` it needs and ignores
the rest, so run `bynkc check` to see what is wrong with one.

## Editor support

A JSON Schema for `bynk.toml` is published at
[`https://bynk-lang.org/schemas/bynk.toml.json`](https://bynk-lang.org/schemas/bynk.toml.json).
It describes every table and key on this page, with each key's type, default
and allowed values, and it flags an unknown table or key the way `bynkc check`
does. The toolchain generates it from the same types that read the manifest
(`cargo xtask manifest-schema`), and CI fails when the committed copy falls
behind them, so it can't drift from what the compiler accepts.

A TOML language server that reads JSON Schemas uses it for completion of
tables, keys and values, hover documentation, and validation:

- **VS Code:** the Bynk extension associates the schema with every
  `bynk.toml`. Install a TOML extension that honours the association, such as
  [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml);
  the Bynk extension suggests it the first time you open a `bynk.toml` without
  one. It is a recommendation, not a requirement.
- **Other editors** (taplo, in Zed, Helix and others): name the schema in a
  comment on the manifest's first line:

```toml
#:schema https://bynk-lang.org/schemas/bynk.toml.json
[project]
name = "my-project"
```

The schema checks a manifest's shape: names, types and allowed values. What
needs the project, such as whether an `[unstable] allow` entry names a real
standard-library item, is still reported by `check` and the language server.
The language server doesn't analyse `bynk.toml` itself yet.

## `[project]`

Project metadata. Neither key affects compilation; both are display only and
have no default (unset stays unset).

| Key | Type | Default | Notes |
|---|---|---|---|
| `name` | string | — | Display name for the project. |
| `version` | string | — | Version string for the project. |

## `[package]`

A shareable project's identity, in place of `[project]`. A manifest with both
is refused, so a project never has two names. Nothing is published or fetched
yet: the name and version are checked now, and `exports` is checked by
`bynk check`.

| Key | Type | Default | Notes |
|---|---|---|---|
| `name` | string | required | `organisation/package`, like `acme/money`. The organisation matches `[a-z][a-z0-9-]*`. The package matches `[a-z][a-z0-9_]*` and isn't a Bynk keyword, because a consumer writes it in source as the start of `<alias>.<unit>`; a hyphenated name is refused with the underscore spelling suggested. Each is at most 64 characters. The organisation `bynk` is reserved for first-party packages. |
| `version` | string | — | Strict semver 2.0, like `1.4.0`. Publishing will require it. |
| `description` | string | — | A one-line description. |
| `license` | string | — | The licence, as an SPDX expression. |
| `repository` | string | — | The source repository's URL. |
| `exports` | array of strings | every commons and adapter | The units the package shares. |

**What a package shares.** A package shares **commons** and **adapters**, and
never a **context**: a context is a deployment unit, and its Worker, secrets
and deploy state belong to the project that deploys it. A package may still
hold contexts, for examples or its own tests; they are never exported.
`bynk check` refuses an `exports` entry that names a context
(`bynk.project.export_context`) or a unit the project doesn't declare
(`bynk.project.export_unknown`).

## `[dependencies]`

The packages this project uses, keyed by **alias**: the name source reaches a
dependency's units by, as `<alias>.<unit>`.

```toml
[dependencies]
money = { package = "acme/money", path = "../packages/money" }
```

```bynk,fragment
commons pricing

uses money.currency
```

Each entry is a table (there is no bare-version shorthand) with `package` and
exactly one source:

| Key | Type | Notes |
|---|---|---|
| `package` | string | Required. The package, as `organisation/package`. It must be the `[package] name` the source holds. |
| `path` | string | A directory holding the package's `bynk.toml`, relative to this project. **Resolved now.** |
| `git` | string | A git repository URL. Needs `rev`. Checked, then refused until packaging slice 2b. |
| `rev` | string | The tag or commit of `git` to use. Only with `git`. |
| `version` | string | A registry version range. It must be pinned: `*`, `latest` and an empty range are refused. Checked, then refused until packaging slice 4, which also checks the range syntax. |

An alias matches `[a-z][a-z0-9_]*`, isn't a Bynk keyword, and isn't `bynk`,
which the standard library keeps. It can't also be the first segment of one of
this project's own units (`bynk.package.alias_shadows_unit`).

**What a dependency shares.** A consumer reaches only the commons and adapters
in the dependency's [`[package] exports`](#package), or all of them when it
lists none (`bynk.package.unit_not_exported`). A dependency's units can use
each other freely, and only the units a consumer actually reaches are
compiled, so adding a dependency you don't use changes nothing that's emitted.
A dependency's contexts and suites are never compiled: a context belongs to
the project that deploys it. Its units are emitted under
`out/deps/<organisation>/<package>/`.

**One copy of each package.** The same package reached through two aliases,
from this project and from another dependency, is one package with one set of
types. The same package reached from two different directories is refused
(`bynk.package.duplicate_source`), and so is a cycle
(`bynk.package.dependency_cycle`).

**A dependency's own path dependencies** are honoured only between members of
one [workspace](#workspace): one that resolves outside the workspace, or any
when there is no workspace, is refused (`bynk.dependency.path_outside_root`).
This project's own `path` entries are honoured wherever they point.

From a dependency's `bynk.toml`, only `[package]`, `[paths]` and
`[dependencies]` are read. Its `[unstable]`, `[workers]`, `[test]` and `[fmt]`
are its own, and this project's settings govern the whole build.

**Not yet.** A dependency can't carry an adapter with npm `requires` (packaging
slice 3) or an `asset` (`bynk.package.unsupported_in_dependency`).

## `[dev-dependencies]`

The packages only this project's tests use. Its entries are written and
checked exactly as in [`[dependencies]`](#dependencies), and a non-empty table
is refused until packaging slice 2b:

```text
bynkc: `[dev-dependencies]` declares dependencies, but they can't be resolved until packaging slice 2b (#843) — remove them for now
```

## `[registries]`

The package registries this project or workspace routes organisations to, one
table per registry under `[registries.<name>]`. They are checked now and used
once packages are fetched; nothing reads a registry yet. In a workspace, only
the root declares them: a member's own `[registries]` is refused.

| Key | Type | Notes |
|---|---|---|
| `index` | string | Required. The index URL: `https://`, or `http://` to `localhost` only. |
| `organisations` | array of strings | The organisations whose packages resolve here. An organisation is listed under one registry at most, and `bynk` never. |
| `mirror-of` | string | The registry this one mirrors: `bynk` (the first-party registry), or another registry declared here that isn't itself a mirror. Not with `organisations`. |
| `key` | string | The registry's pinned signing key, `ed25519:<base64>`. |

A registry name matches `[a-z][a-z0-9-]*`, and `bynk` is refused: it already
means the first-party registry. **Credentials never go in `bynk.toml`**, and a
`token` key is refused. A registry's token is read from
`BYNK_REGISTRY_<NAME>_TOKEN`, its name uppercased with `-` as `_`
(`acme-internal` reads `BYNK_REGISTRY_ACME_INTERNAL_TOKEN`), or from the user
credentials file.

## `[workspace]`

Makes this directory a **workspace root**: a group of projects, its
**members**, that `bynk check`, `bynk fmt` and `bynk test` run across. A
workspace root is not a project. Its `bynk.toml` holds only `[workspace]` and
`[registries]`, and it holds no sources of its own.

| Key | Type | Default | Notes |
|---|---|---|---|
| `members` | array of strings | required | The member directories, relative to the root. A path component may use `*` and `?`; each `*` matches within one directory level (`packages/*`), and `**` isn't supported. |
| `exclude` | array of strings | `[]` | Directories `members` matches that aren't members, as patterns. |

Each member holds its own `bynk.toml` and can't itself be a workspace. A
workspace whose patterns match no member is an error. See
[Workspaces](/book/guides/projects-build-and-deployment/layout/#workspaces)
for which commands run where.

## `[paths]`

The project's source tree. Test-ness is structural — a `suite` is a test
wherever it lives (since v0.113) — so the layout is a flat `include`/`exclude`,
not a source/test role split. Each path is resolved relative to the project root
(the directory containing `bynk.toml`).

| Key | Type | Default | Notes |
|---|---|---|---|
| `include` | array of strings | conventional roots | Trees to compile. Defaults to the conventional roots that exist (`src`, and `tests` when present), or the project root itself when neither does. |
| `exclude` | array of strings | `[]` | Subtrees to skip during discovery (monorepo, vendored, or generated `.bynk`). The tool's own `out`/`node_modules` caches and dot-directories are always skipped. |

A conventional `src/`(+`tests/`) project and a flat project (`.bynk` at the root)
both need no `[paths]` at all. Any other key is an error, including the legacy
`src`/`tests` keys and `out`. The output directory comes from the command line
(`bynkc compile -o`), not the manifest.

### Path consistency

Each **source** unit's file path must align with its qualified name. A
`context commerce.orders` must live at `commerce/orders.bynk` under an `include`
root (or be split across `commerce/orders/*.bynk`). A **`suite`** has no
path-identity requirement — it names its target and is legal in any file.

Misalignment of a source unit is rejected at load time:

- `bynk.project.inconsistent_commons_name` — a source unit's declared name does
  not match its location.

This code is described in the [diagnostic index](/book/reference/diagnostics/).

## `[fmt]`

Formatter settings. Read by **both** format-on-save in the editor and the
`bynkc fmt` / `bynk fmt` CLI, through one shared reader, so a section they can
both read is interpreted identically.

They part company on a section that does **not** read: the CLI reports the
error and formats nothing, while the language server falls back to the
canonical style and keeps serving (it cannot refuse to run). So a manifest with
a typo formats one way in the editor and fails on the command line — run
`bynkc fmt` to see what is wrong with it.

The CLI resolves options in three layers, each overriding the one before: the
canonical defaults below, then this section, then the flags a run passes
(`--indent`, `--indent-width`, `--max-line-width`,
`--trailing-comma`/`--no-trailing-comma`). A flag overrides only the field it
names. `--no-config` skips this section altogether, for a script that wants the
canonical style whatever project it is pointed at.

The manifest is resolved **per file**: the nearest `bynk.toml` at or above the
file being formatted. So `bynk fmt ../other-project/src/x.bynk` obeys the other
project's style, and one command may format files from two projects correctly.

A key this section does not recognise is an error, not a silent no-op — a
`max_line_length = 120` sitting in a manifest while the formatter quietly used
100 is exactly the failure a config layer must not have. See the
[`bynk-fmt` reference](/docs/tooling/bynk-fmt/) and the
[Format your code](/docs/editor-and-tooling/format/) how-to.

| Key | Type | Default | Notes |
|---|---|---|---|
| `indent` | string | `"tab"` | Indentation style: `"tab"` (one tab per nesting level) or `"spaces"`. |
| `indent_width` | integer | — (falls back to `2`) | Spaces per nesting level. Only consulted when `indent = "spaces"`; defaults to `2` in that case. |
| `max_line_width` | integer | `100` | Soft target line width. A construct wider than this wraps across lines where the grammar allows. |
| `trailing_comma` | boolean | `true` | Emit trailing commas in multi-line records, sums, list literals and `exports` clauses. |

## `[lsp]`

Language-server settings, consumed by `bynkc-lsp`. See the
[`bynk-lsp` reference](/docs/tooling/bynk-lsp/).

| Key | Type | Default | Notes |
|---|---|---|---|
| `diagnostics_mode` | string | `"live"` | When diagnostics are computed: `"live"` or `"on_save"`. Any value other than `"on_save"` is treated as `"live"`. |
| `diagnostics_debounce_ms` | integer | `300` | Debounce interval, in milliseconds, for live diagnostics. |

## `[workers]`

Settings for the Cloudflare Workers a Workers build emits (`bynkc compile
--target workers`, `bynk dev`, `bynk deploy`). A bundle build emits no
`wrangler.toml`, so the flags have no effect there, but the table is still
checked, and the duplicate warning below still appears.

| Key | Type | Default | Notes |
|---|---|---|---|
| `compatibility_flags` | array of strings | `[]` | Extra [compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/) for every Worker's `wrangler.toml`, after the default set. |

Every Worker gets `global_fetch_strictly_public` whatever this list says. In
Bynk, a `Fetch` to a URL goes over the public internet, and the flag makes
that true even when the URL is another Worker on the same Cloudflare account.
Without it Cloudflare refuses that fetch (error 1042), so one Bynk service
couldn't `Fetch` another's `workers.dev` URL. Contexts in the same project
reach each other through Service Bindings (`consumes`), which the flag doesn't
change.

The flags you list follow the default, in order:

```toml
[workers]
compatibility_flags = ["nodejs_compat"]
```

```toml
# workers/<context>/wrangler.toml (generated)
compatibility_flags = ["global_fetch_strictly_public", "nodejs_compat"]
```

Flag names aren't checked, so a flag newer than the compiler works, and a
misspelt one is left for Wrangler and the Workers runtime to reject. A flag
listed twice, or one that is already on by default, is dropped with a
`bynk.project.duplicate_compatibility_flag` warning, which `check`, `compile`,
`dev` and `deploy` all report, once per build. Anything but a list of strings
is an error.

## `[unstable]`

Opts in to **experimental** standard-library items. Every first-party item is
stable or experimental. A stable one is under the 1.0 compatibility promise; an
experimental one carries no promise, may change or disappear in any release,
and can't be used until it is listed here.

| Key | Type | Default | Notes |
|---|---|---|---|
| `allow` | array of strings | `[]` | Item paths to allow: a unit (`bynk.cloudflare`), an item it exports (`bynk.cloudflare.Kv`), or a capability operation (`bynk.cloudflare.Kv.list`). An entry covers its path and everything under it. |

```toml
[unstable]
allow = ["bynk.cloudflare.D1"]
```

Using an experimental item that no entry covers is a
`bynk.stability.experimental` error at the use: a `consumes` selection, a
`uses`, an imported function called or passed as a value, or a call to a capability
operation.
Its note names the entry to add.

An entry that names no standard-library item is a
`bynk.project.unstable_unknown_item` error. An entry that reaches nothing
experimental is a `bynk.project.unstable_not_experimental` **warning**: the item
has become stable, so the entry can go. Stabilising an item never breaks a
build. `check`, `compile`, `dev`, `deploy` and the language server all read this
table the same way and report the same diagnostics. Anything but a list of
strings is an error.

No shipped item is experimental yet. The
[first-party capabilities](/book/reference/bynk-capabilities/#stability) page
lists what is.

## `[test]`

Deployments that [`bynk test --against <name>`](/docs/bynk-cli/#bynk-test) may
run the `system` suites against. Each is a table under `[test.targets]`, named
by its key. `--against` accepts only `dev` or one of these names: a bare URL is
refused, so a run can't be pointed at a deployment by accident.

| Key | Type | Default | Notes |
|---|---|---|---|
| `url.<worker>` | string | required | Each Worker's base URL, keyed by its worker-directory name (`shop.orders` is `shop-orders`). At least one is required. |
| `secrets` | array of strings | `[]` | The names of the secrets the deployed Workers verify with. The runner reads each from the **environment** and signs with it; a name that isn't set refuses the run. Never put a secret's value in `bynk.toml`. |
| `vars` | table of strings | `{}` | The non-secret values the target was deployed with (`vars = { PAYMENTS_PROCESSOR = "fake" }`). A case runs against the target only if its `env` agrees with them on every name either one mentions. |
| `production` | boolean | `false` | Marks a production deployment, which `--against` refuses. |

```toml
[test.targets.preview]
url.shop-orders = "https://shop-orders-preview.example.workers.dev"
url.shop-payments = "https://shop-payments-preview.example.workers.dev"
secrets = ["AUTH_JWT_SECRET"]

[test.targets.live]
url.shop-orders = "https://shop-orders.example.com"
production = true
```

`dev` is reserved for the local `bynk dev` the runner starts itself, and isn't
a target name.

## Legacy mode

Without a `bynk.toml`, a single `.bynk` file compiles as a standalone unit (the
[first-program](/book/tutorials/01-first-program/) flow). This is the simplest
way to start.

Project features expect the manifest-driven layout above. In particular
`bynkc test`, and the project-aware [`bynk` driver CLI](/docs/bynk-cli/)
commands — `bynk doctor`, `bynk new`, `bynk dev`, and `bynk deploy` — require a project (a
`bynk.toml`, or a `src/` directory).
