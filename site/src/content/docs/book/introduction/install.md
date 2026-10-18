---
title: Install
---
Bynk is pre-1.0 and is not yet published to a package registry. You install it
by **building from source** with a recent Rust toolchain.

## Prerequisites

- **Rust** (stable, 2024 edition). Install via [rustup](https://rustup.rs/).
- **Git**, to clone the repository.
- A **Node.js / TypeScript** toolchain if you want to type-check or run the
  emitted TypeScript: Node **22 or newer** (`bynk doctor` checks it), and
  `wrangler` if you want to deploy to Cloudflare Workers.

## Build and install the compiler

Clone the repository and install the `bynkc` binary with `cargo`:

```sh
git clone https://github.com/bynk-lang/bynk.git
cd bynk
cargo install --path bynkc
```

This puts `bynkc` on your `PATH` (under `~/.cargo/bin` by default). Verify it:

```sh
bynkc --help
```

## Check your environment with `bynk doctor`

Rather than hunting down `node`, `tsc`, and `wrangler` one broken command at a
time, install the **`bynk` driver** and let it tell you exactly what your
machine is ready for:

```sh
cargo install --path bynk
bynk doctor
```

`bynk doctor` groups its checks by capability — compile/check/fmt, `bynk test`,
`dev`/deploy, editor support — and prints the exact remedy for anything missing.
It is the recommended first step: the prerequisites below are *checked*, not just
listed. See [Check your environment with `bynk
doctor`](/docs/editor-and-tooling/doctor/) for the capability groups, exit
codes, and `--format` outputs.

`bynkc` exposes four commands:

| Command          | Purpose                                            |
|------------------|----------------------------------------------------|
| `bynkc compile`  | Compile Bynk source to TypeScript.                 |
| `bynkc check`    | Type-check without emitting.                        |
| `bynkc fmt`      | Format Bynk source.                                |
| `bynkc test`     | Compile and run `suite` blocks.                     |

See the [CLI reference](/docs/cli/) for every flag and exit code.

## Optional: the language server

For editor integration (diagnostics, hover, go-to-definition), install the
language server:

```sh
cargo install --path bynk-lsp
```

This provides the `bynkc-lsp` binary. Most users consume it through the VS Code
extension rather than invoking it directly — see
[Set up editor support](/docs/editor-and-tooling/editor-support/).

## Dev Containers / Codespaces

In a [Dev Container](https://containers.dev/) or GitHub Codespace, a feature
installs the toolchain for you. Add it, with the official Node feature, to your
`devcontainer.json`:

```jsonc
{
  "image": "mcr.microsoft.com/devcontainers/base:ubuntu-24.04",
  "features": {
    "ghcr.io/devcontainers/features/node:1": { "version": "22" },
    "ghcr.io/bynk-lang/features/bynk:1": {}
  }
}
```

The feature installs `bynk`, `bynkc` and `bynkc-lsp` from one GitHub Release
(checked against that release's `SHA256SUMS`), then `wrangler@4` and
TypeScript 7 with npm. In the finished container `bynk doctor --strict` is all
green, and the VS Code extension, which the feature also adds, uses the
`bynkc-lsp` on `PATH` instead of downloading one. Its options:

| Option       | Default  | Meaning                                                                         |
|--------------|----------|---------------------------------------------------------------------------------|
| `version`    | `latest` | The release to install, such as `v0.330.4`. Pin it for a reproducible image.    |
| `wrangler`   | `true`   | Install `wrangler@4` globally, for `bynk dev` and deploys. Needs Node.          |
| `typescript` | `true`   | Install TypeScript globally, the `tsc` that `bynk test` type-checks with. Needs Node. |

The feature does not install Node itself. It installs after the Node feature,
and fails with a pointer to it if `npm` is missing while `wrangler` or
`typescript` is on. Releases cover Linux on x86_64 and arm64, built on Ubuntu
24.04, so use a base image at least that new.

This repository's own `.devcontainer/` uses the same install script, plus Rust,
for working on Bynk itself.

## Create your first project

With the driver installed, scaffold a complete, runnable project and serve it —
no manifest to write, no layout to memorise:

```sh
bynk new hello
cd hello
bynk dev          # serving on http://localhost:8787
```

`bynk new` only writes files (it needs no toolchain), and the project it writes
is served by `bynk dev` unmodified. See [Start a new
project](/book/guides/projects-build-and-deployment/start-a-project/).

## Next steps

- [Start a new project](/book/guides/projects-build-and-deployment/start-a-project/)
  with `bynk new`.
- [Compile your first program](/book/tutorials/01-first-program/) — what a single
  Bynk file compiles to, by hand.
