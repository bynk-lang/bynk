---
title: "Check your environment with `bynk doctor`"
---
**Goal:** find out whether your machine is ready to compile, test, and deploy
Bynk — before you hit a broken command.

Bynk has a Rust side (the `bynkc` compiler) and a Node side (`tsc`/`tsx` to run
the emitted TypeScript, `wrangler` to deploy to Cloudflare). `bynk doctor`
checks both in one go and prints the exact remedy for anything missing.

> **Understand — the toolchain has two halves.** `bynkc` compiles Bynk to
> TypeScript; that's pure and needs nothing but the compiler. *Running* the
> output (`bynk test`) needs Node and a TypeScript runner; *deploying* needs
> `wrangler`. `bynk doctor` groups its checks along exactly those lines, so you
> are never told you're "unhealthy" for lacking a tool you don't need.

## Run it

```sh
bynk doctor
```

You'll get a grouped report — one block per capability — showing each tool's
**presence, version, and provenance**, with a fix line under anything that
isn't ready:

```text
bynk doctor — environment report
driver: bynk 0.66.0
compiler: in-process

✓ compile [ok]
    compiler — in-process
✓ test [ok]
    node — v22.12.0 (path)
    tsc | tsx — tsc v7.0.2 (path)
! deploy [warn]
    node — v22.12.0 (path)
    wrangler — provisionable via npx (not installed)
      ↳ fix: npm install -g wrangler@4, or clear a stale npx cache (~/.npm/_npx)
· editor [note] (optional)
    bynkc-lsp — missing
      ↳ fix: install bynkc-lsp (or download from releases)
```

## The capability groups

| Capability | Needs | Missing means |
|---|---|---|
| **compile / check / fmt** | nothing — the compiler is built into `bynk` | always available |
| **`bynk test`** | Node ≥ 22.13 **and** `tsc` (TypeScript 5–7) or `tsx` | you can't run `test` blocks |
| **dev / deploy** | Node ≥ 22.13 **and** `wrangler` (4.107.0 or newer for `bynk dev`) | you can't deploy to Cloudflare |
| **editor** *(optional)* | `bynkc-lsp` | a note — editor features only |
| **build-from-source** *(optional)* | a Rust toolchain | shown only inside the Bynk repo |

A `node` below the floor (Node **22.13**: 22 is the oldest major still in
support, and 22.13 its first release to load `node:sqlite` without a flag) is
reported as a warning, `v20.11.0 below floor (≥ 22.13)`, with the fix to install a
newer one. `bynkc test --inspect` checks its own minor floor on top of that: Node
≥ 22.6 to run, ≥ 22.18 for source-mapped breakpoints.

An installed `wrangler` older than **4.107.0** is a warning too:

```text
    wrangler — v4.100.0 (path), below 4.107.0: `bynk dev` can't serve compatibility date 2026-07-01, and agents can't deploy
      ↳ fix: npm install -g wrangler@4
```

Every generated `wrangler.toml` pins a `compatibility_date`, and an older
wrangler's runtime refuses a newer date outright. It doesn't fall back, so
`bynk dev` would fail. `bynk dev` prints the same warning before it serves. A
wrangler that `doctor` would fetch with `npx` can't be versioned without running
it. If `bynk dev` then fails with "This Worker requires
compatibility date …", the npx cache holds an older wrangler: clear
`~/.npm/_npx`, or install one with `npm install -g wrangler@4`.

An older wrangler also can't deploy an agent. Every generated `wrangler.toml`
declares its Durable Object classes in an `exports` map, which wrangler reads
from 4.107.0, so `bynk deploy` refuses to push a context with an agent on an
older one, naming the version and the fix. A project with no agent still
deploys, which is why `doctor` keeps this a warning rather than failing
`--only deploy`.

The emitted TypeScript is verified under **TypeScript 5** (the oldest supported
major) and **7** (the current one, what `npm install -g typescript` installs). A
`tsc` below 5 is a warning, `below floor (≥ 5)`, with the fix
`npm install -g typescript@7`. A `tsc` newer than 7 is reported as
`untested (verified up to 7)` but stays `ok`: it says the emitted output hasn't
been checked under that major yet, not that anything is wrong. So
`bynk doctor --strict` doesn't fail the day a new TypeScript major ships. `tsx`
also satisfies the requirement, but it only runs the tests: it doesn't
type-check them.

### Provenance, and why `npx` isn't "ok"

Each tool reports **where** it was found: on your global `PATH`, in a
project-local `node_modules/.bin`, or only **provisionable via `npx`**. That last
one is a warning, not a pass: `npx --yes` *downloads* the package the first time
you use it, so an environment that "works via npx" still pauses to fetch on first
real use. `doctor` tells you the difference.

### The in-process compiler (and the `BYNK_BYNKC` override)

`bynk` **links the compiler in-process**, so compiling needs no separate `bynkc`
binary — `compile` is always available, and there is no version to drift. Power
users can still point `bynk` at an *external* compiler with the **`BYNK_BYNKC`**
environment variable (e.g. to pin a specific version); only then does `doctor`
resolve that binary and report **driver↔compiler skew** — a minor drift warns, a
major drift is an error. With no override there is no second compiler to check.

## Exit codes — for scripts and CI

The exit code depends on **what you asked about**:

- **Bare `bynk doctor`** is informational. It surveys everything but only fails
  if the compile floor is broken — and with the in-process compiler that only
  happens under a broken `BYNK_BYNKC` override — so a compile-only user exits `0`
  even without Node or `wrangler`.
- **`bynk doctor --only <capability>`** gates on one capability. `bynk doctor
  --only deploy` exits non-zero on a machine that genuinely can't deploy.
- **`bynk doctor --strict`** turns *every* warning — optional gaps, `npx`
  provisionability, minor skew — into a failure. Use it for an all-green CI gate.

## Machine-readable output

Two formats are a stable, scriptable contract:

```sh
bynk doctor --format short    # one `capability: level (remedy)` line per row
bynk doctor --format json     # structured, for CI
```

```text
compile: ok
test: ok
deploy: warn (npm install -g wrangler@4, or clear a stale npx cache (~/.npm/_npx))
editor: note (install bynkc-lsp (or download from releases))
```

`doctor` only **reports** — it never installs anything. Copy the fix line it
prints and run it yourself.

## From VS Code

**Bynk: Doctor** in the [VS Code extension](/docs/tooling/vscode-bynk/) runs
`bynk doctor --format json` in your project and shows the report as a checklist,
grouped by capability. Each gap carries its remedy, with buttons to run its
command in a terminal (after you confirm it), open a link or the relevant
setting, or copy it. The extension doesn't run the doctor on its own. A test
run, debug session or dev server that fails on a missing tool offers **Run
Doctor**, scoped with `--only` to what failed. If `bynk` itself can't be found,
the doctor can't run either, so the extension says how to install it instead.

## Related

[`bynk dev`](/book/guides/projects-build-and-deployment/run-locally/) pre-flights the
same **deploy** capability (Node + `wrangler`) before it serves, so a green
`bynk doctor --only deploy` is exactly what `dev` needs to run.
