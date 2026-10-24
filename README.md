# Bynk

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust 2024 (MSRV 1.95)](https://img.shields.io/badge/rust-2024%20edition%20%C2%B7%20MSRV%201.95-orange.svg)](rust-toolchain.toml)

**Bynk** is a statically typed, *architecture-first* programming language for
building services. The shape of a program — its contexts, services, agents, and
the types that flow between them — is part of the language, not a convention
layered on top. Bynk compiles to **typed TypeScript** and targets **Cloudflare
Workers**.

> ⚠️ Bynk is **pre-1.0 and under active development.** The language evolves in
> small, spec-first increments. The [Bynk Book](https://bynk-lang.org/book/) documents *what compiles
> today*; planned features are marked as planned.

## The idea in one example

```bynk
context greet

service api from http {
  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("pong")
  }
}
```

Compiling this with `bynkc` produces TypeScript you can read, run, and deploy —
the router, boundary validation, and the Worker entry point are generated for
you.

## What makes Bynk distinct

- **Make illegal states unrepresentable.** *Refined types* (types carrying a
  predicate), *opaque types*, and errors-as-values (`Result`, `Ok`/`Some`/
  `None`) let whole classes of bug go unexpressed.
- **Architecture in the language.** Contexts, services, and stateful *agents*
  are first-class, and a `context` is the unit of deployment — it becomes one
  Cloudflare Worker.
- **Honest effects and capabilities.** Dependencies are declared in signatures
  (`given Logger`), supplied by the platform, and mockable in tests.
- **Compiles to TypeScript.** You get JavaScript-ecosystem interop and a natural
  fit for Cloudflare Workers, with a static type system in front of it.
- **Testing is built in.** `suite`/`case` blocks, `expect`, and `Val[T]` value
  fabrication ship with the language.

## Install

Bynk is not yet on a package registry for end users; install it by building from
source with a recent Rust toolchain (stable, 2024 edition — see
[rustup](https://rustup.rs/)).

```sh
git clone https://github.com/bynk-lang/bynk.git
cd bynk
cargo install --path bynkc      # the `bynkc` compiler
cargo install --path bynk       # the `bynk` driver (doctor / new / dev / deploy, and more)
cargo install --path bynk-lsp   # optional: the `bynkc-lsp` language server
```

That builds `main`, which moves on with every merged increment. To build the
last release instead, check out its tag (the newest `vX.Y.Z` on the
[releases page](https://github.com/bynk-lang/bynk/releases)) before installing.
Before 1.0, a release is cut when a track of work retires ([release
discipline](design/bynk-release-discipline.md)).

`bynkc --help` lists the four compiler commands (`compile`, `check`, `fmt`,
`test`); `bynk --help` lists the driver's (`doctor`, `dev`, `deploy`, `new`,
`check`, `fmt`, `test`, `explain`).

## Quick start

Scaffold a complete, runnable project and serve it — three commands from nothing
to a running service on `http://localhost:8787`:

```sh
bynk new hello       # scaffold bynk.toml + src/hello.bynk
cd hello
bynk dev             # compile and serve it locally
```

`bynk new` only writes files (no toolchain needed), and `bynk dev` serves what it
wrote unmodified. Prefer a worked example? The bundled
[`examples/hello-world`](examples/hello-world/) is a complete project you can also
`bynkc check src`, `bynkc test .`, and deploy.

See [Start a new project](https://bynk-lang.org/book/guides/projects-build-and-deployment/start-a-project/)
or [Compile your first program](https://bynk-lang.org/book/tutorials/01-first-program/).

## Repository layout

This is a Cargo workspace. The three user-facing tools — `bynkc`, `bynk`, and
`bynk-lsp` — are front-ends over a layered set of library crates; every crate
except `bynk-wasm`, `bynk-testkit`, `tree-sitter-bynk`'s Rust binding, `xtask`,
and `fuzz` is published to crates.io at each release. Each crate's README
says where it sits in the layering.

| Path | What it is | Published as |
| ---- | ---------- | ------------ |
| [`bynkc/`](bynkc/) | The `bynkc` compiler CLI (`compile` / `check` / `fmt` / `test`) — a thin front-end over the library crates. | [crates.io](https://crates.io/crates/bynkc) |
| [`bynk/`](bynk/) | The `bynk` driver — links the compiler in-process and orchestrates the Node toolchain (`doctor` / `new` / `dev` / `deploy`, plus `check` / `fmt` / `test` / `explain`). | [crates.io](https://crates.io/crates/bynk) |
| [`bynk-lsp/`](bynk-lsp/) | The `bynkc-lsp` Language Server (diagnostics, hover, go-to-definition, …). | [crates.io](https://crates.io/crates/bynk-lsp) |
| [`bynk-syntax/`](bynk-syntax/), [`bynk-std/`](bynk-std/), [`bynk-project/`](bynk-project/), [`bynk-check/`](bynk-check/), [`bynk-ir/`](bynk-ir/), [`bynk-lower/`](bynk-lower/), [`bynk-ts/`](bynk-ts/), [`bynk-emit/`](bynk-emit/), [`bynk-strip/`](bynk-strip/), [`bynk-render/`](bynk-render/), [`bynk-fmt/`](bynk-fmt/), [`bynk-ide/`](bynk-ide/), [`bynk-driver/`](bynk-driver/) | The compiler's library crates: syntax, the standard library's first-party sources (as data), the project model, checking, the declaration IR, TypeScript emission and stripping, diagnostic rendering, formatting, editor analysis, and the command bodies the CLIs share. | crates.io |
| [`bynk-grammar/`](bynk-grammar/) | Renders the tree-sitter grammar to EBNF for the book's grammar reference. | [crates.io](https://crates.io/crates/bynk-grammar) |
| [`bynk-wasm/`](bynk-wasm/) | The compiler as a wasm module, backing the [playground](https://github.com/bynk-lang/playground) and the VS Code web extension. | — |
| [`tree-sitter-bynk/`](tree-sitter-bynk/) | The tree-sitter grammar — the source of truth for syntax highlighting. | npm |
| [`vscode-bynk/`](vscode-bynk/) | The VS Code extension (launches the language server). | — |
| [`site/`](site/) | The Bynk Book (Astro + Starlight): tutorials, how-to guides, reference, and the normative spec. Published at [bynk-lang.org](https://bynk-lang.org/book/). | — |
| [`design/`](design/) | Internal design notes and decision records (ADRs). | — |
| [`examples/`](examples/) | Example projects. | — |
| [`bynk-testkit/`](bynk-testkit/), [`xtask/`](xtask/), [`fuzz/`](fuzz/) | Internal: cross-crate test fixtures, repo automation (`cargo xtask`), and fuzz targets. | — |

## Developing

Run the CI gates locally before you push — CI's critical path is the Windows
test leg at roughly seven minutes, so a formatting slip caught here saves a full
round trip:

```sh
cargo xtask ci          # formatting, clippy, and the test suite
cargo xtask ci --fast   # just the two gates that need no compile-and-link
```

`--fast` is what the opt-in `pre-push` hook runs. Enable it once per clone:

```sh
git config core.hooksPath .githooks
```

Note that `--fast` is not compile-free: clippy skips codegen and linking, not
compilation. It is about three seconds on a warm cache, roughly a minute once
every crate's fingerprint has changed, and several minutes on a fresh clone —
so expect a wait the first time, and after a lockfile, toolchain or `[profile]`
change. `git push --no-verify` skips the hook when that is not a good moment.

The gates lint your working tree, not the commits being pushed, so a push of an
older branch or one made with dirty local edits checks something other than
what lands. CI remains the authority — `cargo xtask ci` front-runs it, it does
not replace it.

## Documentation

The **[Bynk Book](https://bynk-lang.org/book/)** is the canonical guide and
reference. It follows [Diátaxis](https://diataxis.fr/), grouped concern-first so
each topic keeps its explanation, recipes, and reference together:

- **[Tutorials](https://bynk-lang.org/book/tutorials/01-first-program/)** — learn Bynk by building.
- **[Guides](https://bynk-lang.org/book/guides/)** — task-focused recipes, each section
  opening with the *why* before the *how*.
- **[Reference](https://bynk-lang.org/book/reference/)** — exact behaviour, including the
  [normative spec](https://bynk-lang.org/book/spec/) and [CLI reference](https://bynk-lang.org/docs/cli/).

The online Book's source lives in [`site/`](site/). Build it locally with
[Astro](https://astro.build/): `cd site && npm install && npm run dev`.

A separate, print-first narrative manuscript, *Architecture that compiles*, is
being developed in
[accuser/architecture-that-compiles](https://github.com/accuser/architecture-that-compiles).
It draws on the documentation as research but does not share its prose or replace
the online guide and reference. Its example programs are compile-tested against a
published Bynk release, so a diagnostic rename here can invalidate a listing
there — that repository's nightly gate is what reports it.

## Status

Bynk is pre-1.0. Some designed features (events, sagas, storage kinds) are
**deferred, not missing**, and land in later increments. See the
[status and roadmap](design/bynk-status-and-roadmap.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any additional
terms or conditions.
