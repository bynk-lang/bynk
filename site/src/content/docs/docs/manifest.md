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
build.

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
```

Delete any line you are happy to leave at its default — the file above is
equivalent to an empty `bynk.toml`, which is equivalent to `[project]` alone.

## Unknown tables and keys

`bynk.toml` holds the six tables below, and nothing else. An unknown table, a
key outside any table (including a table's own name given a plain value, such
as `paths = "src"`), or an unknown key in `[project]`, `[paths]`, `[lsp]` or
`[workers]` is an error. `bynkc` and `bynk` report it, with the nearest name
when there is one, and build nothing. An unknown `[fmt]` key is reported by the
formatter (`bynkc fmt`, `bynk fmt`), which owns that table, not by `check` or
`compile`:

```text
bynkc: `bynk.toml` has no table named `[pahts]` — did you mean `[paths]`?
bynkc: `[project]` has no key named `nmae` — did you mean `name`?
```

A table for a feature that is planned but not built (`[dependencies]`,
`[workspace]`, `[deploy]`) gets the same error, naming the issue that tracks
the feature, so it can't read as working. The language server doesn't refuse
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

## Legacy mode

Without a `bynk.toml`, a single `.bynk` file compiles as a standalone unit (the
[first-program](/book/tutorials/01-first-program/) flow). This is the simplest
way to start.

Project features expect the manifest-driven layout above. In particular
`bynkc test`, and the project-aware [`bynk` driver CLI](/docs/bynk-cli/)
commands — `bynk doctor`, `bynk new`, `bynk dev`, and `bynk deploy` — require a project (a
`bynk.toml`, or a `src/` directory).
