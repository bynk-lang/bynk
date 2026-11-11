---
title: Lay out a project
---
**Goal:** structure a multi-file project with source and tests, and build it.

A project is a directory containing a `bynk.toml` manifest and your `.bynk`
files. The conventional layout keeps source and tests in sibling trees:

```text
my-project/
├── bynk.toml
├── src/
│   ├── counters.bynk       # context counters
│   └── quantities.bynk     # commons quantities
└── tests/
    ├── counters.bynk       # suite counters
    └── quantities.bynk     # suite quantities
```

This is convention, not a rule. **Test-ness is structural** — a `suite` block is
a test wherever it lives — so you can put tests beside the code they exercise, or
even in the *same file*:

```bynk,fragment
// src/quantities.bynk — an atomic file: the commons and its tests together
commons quantities {
	fn double(n: Int) -> Int {
		n + n
	}
}

suite quantities {
	case "doubles" {
		expect double(3) == 6
	}
}
```

When you build, the `suite` is **stripped** — never type-checked for the build,
never emitted to the deployable — while `bynk test` compiles and runs it. There
is no `.test.bynk` suffix: every file is just `.bynk`, and the `suite` keyword is
what marks a test.

## The manifest

`bynk.toml` names the project. Layout config is optional:

```toml
[project]
name = "my-project"
version = "0.1.0"

[paths]
# Both keys are optional. `include` defaults to the conventional roots that
# exist (`src`, and `tests` when present), or the project root itself — so a
# conventional or a flat project needs no `[paths]` at all.
exclude = ["vendor"]   # skip subtrees (monorepo, vendored, generated .bynk)
```

See the [`bynk.toml` reference](/docs/manifest/) for every key. The tool always
skips its own `out/` and `node_modules/` caches and dot-directories.

## Path identity

A **source** unit's path must match its qualified name. A file declaring
`context counters` must be `counters.bynk`; one declaring `context commerce.orders`
must be `commerce/orders.bynk` (under an `include` root). A **`suite`** has no such
requirement — it names its target, so it is legal in any file.

> Source mismatches are reported as `bynk.project.inconsistent_commons_name`.

## Build and test

```sh
bynk compile . --output out      # compile the project (suites stripped)
bynk test .                      # compile and run the tests
bynk check .                     # type-check only
```

## Where a project ends

A subdirectory with its own `bynk.toml` is **another project**, and a project never
reaches into one. Discovery skips it as it skips `out/`, so its sources, fixtures
and migrations stay its own. When the skipped project holds `.bynk` files, the
command says so in one line:

```text
bynk: note: skipped `vendor/inner`: it has its own bynk.toml, so it is a separate project; run the command inside it
```

A directory that is **not** a project, with no `bynk.toml` here or in any
directory above, but one or more projects below it (a repository of independent
projects, say), has no single program to compile. `bynk check` and `bynk test`
refuse there, name the projects (the first five, then a count), and exit non-zero:

```text
bynk: `.` is not a project (no `bynk.toml` here or above) but holds 3 projects that have their own: `a`, `b`, `c`.
Run the command inside one of them, pass `--each` to run it over each, or add a `bynk.toml` here with

    [workspace]
    members = ["*"]

to keep that choice in the repository
```

You have three ways forward. Run the command inside one project. Pass `--each`
to run it over every project in the directory this once. Or add that root
`bynk.toml` to make the directory a [workspace](#workspaces) for good.
`bynk fmt` is the exception: it formats a directory of projects as one tree,
each file by its own project's `[fmt]` settings, since formatting reads no
fixtures or migrations.

### Running a directory of projects with `--each`

`bynk check --each` and `bynk test --each` treat such a directory as an
**implicit workspace**: each project directly inside it runs as a member would
at a workspace root, under a `── alpha ──` header, every project runs even if an
earlier one fails, and the exit code is non-zero if any does.

```console
$ bynk test --each
── alpha ──
…
── beta ──
…
```

- The projects are the **immediate subdirectories** that hold a `bynk.toml`, in
  name order: exactly what `members = ["*"]` would select. A project inside a
  project belongs to that project; `out`, `node_modules` and hidden directories
  are never projects.
- A subdirectory with `.bynk` files but no `bynk.toml` is **not** a project: it
  is skipped, with a one-line note, rather than failing the run as a real
  `members = ["*"]` would. If projects sit deeper, `--each` names them and asks
  you to point it at the directory that holds them.
- The flag is **opt-in**, so a command typed in the wrong directory never
  quietly runs many projects. It does nothing at a workspace root (which
  already runs every member), and is an error at a project, inside one, or on a
  file.
- Each project stands alone, with its own lock and dependencies, as when run by
  itself. Code sharing between members, including a dependency's own path
  dependencies, is what a real [workspace](#workspaces) is for.
- `build`, `compile`, `deploy` and the other commands that act on one project
  do not take `--each`.

An editor needs none of this: the language server already treats each project in
an opened folder as its own, whether or not the folder has a root `bynk.toml`.

## Workspaces

A **workspace** groups several projects in one repository. Its root holds a
`bynk.toml` with a `[workspace]` table naming its **members**, and nothing
else of a project's: no `[project]` or `[package]`, and no sources.

```text
shop/
├── bynk.toml               # [workspace] members = ["packages/*", "apps/shop"]
├── apps/
│   └── shop/
│       ├── bynk.toml       # [project] name = "shop"
│       └── src/
└── packages/
    ├── money/
    │   ├── bynk.toml       # [package] name = "acme/money"
    │   └── src/
    └── fx/
        ├── bynk.toml
        └── src/
```

```toml
[workspace]
members = ["packages/*", "apps/shop"]
exclude = ["packages/scratch"]
```

Each member is an ordinary project with its own `bynk.toml`. A member is any
directory a `members` pattern matches (`*` matches within one directory
level) that no `exclude` pattern matches. A project nested under the root that
no pattern lists isn't a member. A workspace whose patterns match no member is
an error.

**Which commands run where.**

- **At the root**, `bynk check`, `bynk fmt .` and `bynk test` run over every
  member in path order. Every member runs even when an earlier one fails, and
  the command fails if any member does.
  - `bynk check` and `bynk test` head each member's report with its path
    (`── packages/money ──`). `bynk check --format short` adds no headers, and
    its paths are relative to the root, so each line still opens in an editor.
  - `bynk fmt` names files relative to the root.
  - `bynk test --format json`, `--output` and `--against` each belong to one
    project, so they are refused at the root.
  - `bynk dev` and `bynk deploy` run one project, and refuse at the root: run
    them inside a member.
- **Inside a member**, every command behaves exactly as in a standalone
  project. Deploy state (`bynk.deploy.lock`) stays per member.

A workspace root may also hold `[registries]`, which then apply to every
member; a member's own `[registries]` is refused, and so is a member's own
`[workspace]`. A workspace has no name and doesn't appear in any package's
identity: it is a build grouping only.

## Related

- [Write tests and stub collaborators](/book/guides/testing/write-tests/).
- [Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).
- Reference: [`bynk.toml` manifest](/docs/manifest/).
