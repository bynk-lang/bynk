# bynkc

[![crates.io](https://img.shields.io/crates/v/bynkc.svg)](https://crates.io/crates/bynkc)
[![docs.rs](https://img.shields.io/docsrs/bynkc)](https://docs.rs/bynkc)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **Bynk compiler CLI** — the `bynkc` command-line tool (and the library
behind it). It takes [Bynk](https://github.com/bynk-lang/bynk) source,
type-checks it, and emits typed TypeScript targeting Cloudflare Workers.

Bynk is a statically typed, architecture-first language: contexts, services,
agents, refined types, and capabilities are part of the language. See the
[Bynk Book](https://bynk-lang.org/book/) for the full guide
and reference.

## Pipeline

```text
lex  →  parse  →  resolve  →  check  →  emit
```

`bynkc` is a thin front-end: it owns the CLI and the compile/diagnose glue, and
the pipeline itself lives in a layered set of library crates it depends on and
partly re-exports from (so `bynkc::Platform`, `bynkc::compile_project`, …
resolve — but not a crate's whole module tree, e.g. `bynk_syntax::ast` stays
`bynk_syntax::ast`, not `bynkc::ast`):

- [`bynk-syntax`](https://crates.io/crates/bynk-syntax) — lexer, parser, AST,
  spans, the `CompileError` type, and the `bynk.*` diagnostic-code registry.
- [`bynk-project`](https://crates.io/crates/bynk-project) — the project model:
  discovery, the unit dependency graph, and path resolution.
- [`bynk-check`](https://crates.io/crates/bynk-check) — name resolution, type
  checking, the kernel/builtin registries, first-party sources, and actors.
- [`bynk-model`](https://crates.io/crates/bynk-model) — the system model: the
  declaration-level facts the emitter and the editor read, and the builder that
  makes them.
- [`bynk-ts`](https://crates.io/crates/bynk-ts) — the TypeScript tree and its
  printer.
- [`bynk-emit`](https://crates.io/crates/bynk-emit) — build orchestration
  (`project`) and the TypeScript emitter.
- [`bynk-strip`](https://crates.io/crates/bynk-strip) — TypeScript →
  JavaScript type-stripping, behind `--emit js`.
- [`bynk-render`](https://crates.io/crates/bynk-render) — the shared diagnostic
  renderer ([`ariadne`](https://crates.io/crates/ariadne) human + `short`).
- [`bynk-fmt`](https://crates.io/crates/bynk-fmt) — the formatter, behind
  `bynkc fmt`.
- [`bynk-driver`](https://crates.io/crates/bynk-driver) — the command bodies
  `bynkc` shares with the `bynk` driver (`check`, `fmt`, `test`, output writing).

Every error carries a dotted category (`bynk.parse.expected_token`,
`bynk.types.invalid_regex`, …), a source span, and a primary message; many carry
secondary labels and notes.

## Install

```sh
cargo install bynkc
```

Or build from the workspace:

```sh
cargo build --release -p bynkc   # → target/release/bynkc
```

Requires a stable Rust toolchain, 2024 edition (MSRV 1.95).

## CLI

```sh
bynkc check   <input> [--format rich|short]   # type-check without emitting
bynkc compile <input> -o <output>             # emit TypeScript (or JavaScript)
              [--target bundle|workers] [--platform cloudflare|node|browser]
              [--emit ts|js]
bynkc fmt     [inputs...] [--check]           # format in place (or `-` for stdin)
              [--indent tab|spaces] [--indent-width N] [--max-line-width COLUMNS]
              [--trailing-comma | --no-trailing-comma] [--no-config]
bynkc test    [project] [--format rich|json]  # compile and run `suite`/`case` tests
              [--no-run] [--case NAME] [--seed HEX] [--coverage] [--inspect]
```

`<input>` is either a single-file commons (`foo.bynk`) or a project directory
containing a `bynk.toml`. The `workers` target emits one Cloudflare Worker per
context, complete with router, dependency wiring, the shared runtime, and a
`wrangler.toml`. `--emit js` writes the same modules with their types stripped,
so the output runs with no `tsc` in the loop. `bynkc fmt` reads a project's
`[fmt]` section from `bynk.toml` unless `--no-config` is passed. `bynkc test`
needs `node` and `tsc` on `PATH`.

Run `bynkc <command> --help` for every flag, and see the
[CLI reference](https://bynk-lang.org/docs/cli/).

## Library

```rust
use bynkc::{BuildTarget, CompileOptions, compile_project};

// The build never touches disk: `sources` maps every `.bynk` file under the
// root (by canonical path) to its text. The result is an in-memory tree of
// TypeScript files.
let options = CompileOptions::single("path/to/src")
    .sources(sources)
    .target(BuildTarget::Workers);
let output = compile_project(&options)?;
```

To compile a project from disk the way the CLI does — reading `bynk.toml` and
every source — build the options with
[`bynk-driver`](https://crates.io/crates/bynk-driver)'s `project_options`.

The crate re-exports a small set of items from the layers below — `CompileError`
and `Severity` from `bynk-syntax`, `Platform`, `CompileOptions`,
`compile_project`, the renderers, and the strip entry points — not those
layers' modules; `bynkc::ast`, `bynkc::resolver`, `bynkc::checker`,
`bynkc::emitter`, and `bynkc::project` are not part of the published API. The single-string
`compile` entrypoint handles a self-contained commons; the `compile_project`
family handles multi-file projects, build targets, and platforms. To depend on
just one layer, use the individual crate (e.g.
[`bynk-syntax`](https://crates.io/crates/bynk-syntax) to lex/parse without the
checker, or [`bynk-check`](https://crates.io/crates/bynk-check) for
`bynk_check::checker::Ty` and the rest of the semantic-analysis surface). See
the [API docs](https://docs.rs/bynkc).

## Tests

```sh
cargo test -p bynkc
```

The end-to-end harness in `tests/` runs fixture-driven positive and negative
cases. Set `BYNK_REQUIRE_TSC=1` to additionally type-check the emitted
TypeScript with `tsc` (CI does this).

## The language

The normative definition of the language this compiler accepts is the
specification in
[the normative spec](https://bynk-lang.org/book/spec/)
(rendered in the Bynk Book), kept current per increment. The decisions behind
the increments are recorded in
[`design/decisions/`](https://github.com/bynk-lang/bynk/tree/main/design/decisions).

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
