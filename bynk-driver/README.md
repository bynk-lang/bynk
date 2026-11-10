# bynk-driver

[![crates.io](https://img.shields.io/crates/v/bynk-driver.svg)](https://crates.io/crates/bynk-driver)
[![docs.rs](https://img.shields.io/docsrs/bynk-driver)](https://docs.rs/bynk-driver)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **shared front-end of the `bynk` CLI**, for the
[Bynk](https://github.com/bynk-lang/bynk) compiler.

This crate holds the command bodies the `bynk` CLI runs (`check`, `compile`,
`fmt`, `test`) — project rooting, project-failure flattening, and output
writing — parameterised by the program name that prefixes messages.

It holds:

- `discovery` / project rooting — picks project vs. single-tree mode the same
  way for every command that needs it (`check`, `compile`, `test`, `dev`).
- `output` — `write_output`/`write_document`, the real filesystem write
  boundary; the one place emitted `Document::Ts` artefacts are printed
  through `bynk-ts`'s printer (the only code that writes a character).
- `test_json` / `test_runner` / `coverage` — the shared `test` subcommand's
  JSON report shape, runner, and V8-coverage parsing.
- `probe` — toolchain detection (`tsc`/`node` on `PATH`).
- `schema_lock` — the shared `bynk.schema.lock` read/write path both CLIs use.

## Where it sits

This crate depends on [`bynk-emit`](https://crates.io/crates/bynk-emit),
[`bynk-fmt`](https://crates.io/crates/bynk-fmt), and
[`bynk-ts`](https://crates.io/crates/bynk-ts):

```text
bynk-syntax                lexer, parser, AST, CompileError, diagnostic codes
├── bynk-project           project model: discovery, unit graph, paths
├── bynk-ts                the TypeScript tree and its printer
├── bynk-fmt               the formatter
└── bynk-check             name resolution and type checking   + project, std, ts
    ├── bynk-model         the system model and its builder
    ├── bynk-emit          TS emission from a checked program + ts, model, project
    │   └── bynk-driver    shared CLI command bodies, build sequencing + fmt, project, ts, model, strip
    └── bynk-ide           non-bailing editor analysis         + project, fmt, model
bynk-std                   first-party sources and bindings, as data
bynk-strip                 TS → JS type-stripping (text level), on oxc alone
```

Each crate depends on its parent in the tree, plus any crates listed after
its `+`. The front-ends sit on top: the `bynk` CLI over
`bynk-driver`, the `bynk-lsp` language server (`bynk-lsp`) over `bynk-ide`, and
the unpublished `bynk-wasm` playground module over `bynk-driver`
and `bynk-ide`.

The `bynk` binary is the front-end built on top of it.

## Use

`bynk-driver` is consumed by the `bynk` binary directly; it has
no standalone CLI of its own.

```toml
[dependencies]
bynk-driver = "0.538"
```

See the [API docs](https://docs.rs/bynk-driver) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
