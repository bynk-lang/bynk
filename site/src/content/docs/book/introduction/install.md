---
title: Install
---
Bynk is pre-1.0 and is not yet in a package registry, but every
[GitHub Release](https://github.com/bynk-lang/bynk/releases) ships prebuilt
binaries. Download one (below), or [build from source](#build-from-source) with
a recent Rust toolchain, or, in a container, use the
[Dev Container feature](#dev-containers--codespaces).

## Download a release binary

Each release has one archive per platform, holding the driver `bynk`, the
compiler `bynkc` and the language server `bynkc-lsp`, all from the same
build:

| Platform              | Archive                                         |
|-----------------------|-------------------------------------------------|
| Linux x86_64          | `bynk-<tag>-x86_64-unknown-linux-gnu.tar.gz`    |
| Linux arm64           | `bynk-<tag>-aarch64-unknown-linux-gnu.tar.gz`   |
| macOS Intel           | `bynk-<tag>-x86_64-apple-darwin.tar.gz`         |
| macOS Apple silicon   | `bynk-<tag>-aarch64-apple-darwin.tar.gz`        |
| Windows x86_64        | `bynk-<tag>-x86_64-pc-windows-msvc.zip`         |

`<tag>` is the release tag, such as `v0.330.4`. The Linux binaries are built on
Ubuntu 24.04 and need a glibc at least that new. Each release also has a
`SHA256SUMS` file covering every archive. Check the archive against it before
you unpack it. On Linux, for example:

```sh
tag=v0.330.4
target=x86_64-unknown-linux-gnu
base=https://github.com/bynk-lang/bynk/releases/download/$tag
curl -fLO "$base/bynk-$tag-$target.tar.gz"
curl -fLO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS   # must print "OK"
tar -xzf "bynk-$tag-$target.tar.gz"
```

On macOS use `shasum -a 256 --check --ignore-missing SHA256SUMS`. On Windows,
compare `(Get-FileHash bynk-<tag>-x86_64-pc-windows-msvc.zip).Hash` with the
archive's line in `SHA256SUMS`, then extract the `.zip`. Releases also carry
signed build provenance, which `gh attestation verify <archive> --repo
bynk-lang/bynk` checks.

Put the three binaries on your `PATH`. On Linux and macOS, for example:

```sh
install -m 0755 "bynk-$tag-$target"/bynk "bynk-$tag-$target"/bynkc \
  "bynk-$tag-$target"/bynkc-lsp ~/.local/bin/
bynk --version
```

Any directory on your `PATH` works; the example assumes `~/.local/bin` is on
it. On Windows, move `bynk.exe`, `bynkc.exe` and `bynkc-lsp.exe` into a folder
and add that folder to `Path`.

## Prerequisites for testing and deploying

`bynkc` compiles on its own. To type-check or run the emitted TypeScript you
also need Node **22 or newer** and TypeScript (`tsc`), plus `wrangler` to
deploy to Cloudflare Workers. You don't have to work out which you're missing:
`bynk doctor`, below, checks them and prints the command that fixes each gap.

## Check your environment with `bynk doctor`

Rather than hunting down `node`, `tsc`, and `wrangler` one broken command at a
time, let the **`bynk` driver** tell you exactly what your machine is ready
for:

```sh
bynk doctor
```

`bynk doctor` groups its checks by capability — compile/check/fmt, `bynk test`,
`dev`/deploy, editor support — and prints the exact remedy for anything missing.
It is the recommended first step: the prerequisites above are *checked*, not just
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

## Editor support

`bynkc-lsp`, the language server, is in the release archive. Most users run it
through the VS Code extension rather than directly. Install it from the
[Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=bynk-lang.bynk-vscode) (`bynk-lang.bynk-vscode`). It uses the
`bynkc-lsp` on your `PATH`, or downloads the matching release if there isn't
one. It does the same for `bynk` and `bynkc`, so with the extension you need no
separate install or `cargo`: the downloaded tools go on the `PATH` of its
integrated terminals. See [Set up editor support](/docs/editor-and-tooling/editor-support/).

## Build from source

To build instead, you need **Rust** (stable, 2024 edition, via
[rustup](https://rustup.rs/)) and **Git**. Clone the repository and install the
driver, the compiler and the language server with `cargo`:

```sh
git clone https://github.com/bynk-lang/bynk.git
cd bynk
cargo install --path bynk
cargo install --path bynkc
cargo install --path bynk-lsp
```

This puts the binaries on your `PATH` (under `~/.cargo/bin` by default). Then
run `bynk doctor` as above. A build from `main` includes unreleased changes;
the [changelog](/book/reference/changelog/) lists what each version adds.

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
green. The feature also lists the VS Code extension (`bynk-lang.bynk-vscode`),
so VS Code installs it from the Marketplace into the container. It uses the
`bynkc-lsp` on `PATH` instead of downloading one.

The feature's options:

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
