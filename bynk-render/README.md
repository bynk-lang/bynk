# bynk-render

[![crates.io](https://img.shields.io/crates/v/bynk-render.svg)](https://crates.io/crates/bynk-render)
[![docs.rs](https://img.shields.io/docsrs/bynk-render)](https://docs.rs/bynk-render)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **shared diagnostic-rendering layer for the
[Bynk](https://github.com/bynk-lang/bynk) compiler**.

Given a slice of `bynk_syntax::CompileError` plus the source and a filename, it
produces the human and machine forms of a Bynk diagnostic:

- **human** — rich, source-pointing [`ariadne`](https://crates.io/crates/ariadne)
  output (with a colourless variant for byte-stable transcripts), and
- **`short`** — one terse `path:line:col: severity[category]: message` line per
  error, the format the editor problem-matcher parses.

The per-line pieces (`short_line`, `severity_word`) are public too, so a
front-end composing its own line output stays byte-identical to `short`.

Both CLI front-ends render through this one crate, so `bynkc` and `bynk`
display the same error identically — and the `short` form is what the VS Code
problem-matcher parses. The crate is a
pure presentation layer: it depends on
[`bynk-syntax`](https://crates.io/crates/bynk-syntax) **only** (plus `ariadne`)
and never sees the checker or emitter — structured diagnostics flow *down* into
it, never the other way.

## Where it sits

`bynk-render` sits directly on the `bynk-syntax` leaf:

```text
bynk-syntax                lexer, parser, AST, CompileError, diagnostic codes
├── bynk-project           project model: discovery, unit graph, paths
├── bynk-ts                the TypeScript tree and its printer
├── bynk-render            diagnostic rendering
├── bynk-fmt               the formatter
└── bynk-check             name resolution and type checking   + project, std, fmt
    ├── bynk-ir            declaration-level IR
    │   └── bynk-lower     AST → IR helpers
    ├── bynk-emit          build sequencing, TS emission       + ts, ir, lower, project
    │   ├── bynk-strip     TS → JS type-stripping              + ts
    │   └── bynk-driver    shared CLI command bodies           + fmt, render, ts
    └── bynk-ide           non-bailing editor analysis         + project, fmt
bynk-std                   first-party sources and bindings, as data
```

Each crate depends on its parent in the tree, plus any crates listed after
its `+`. The front-ends sit on top: the `bynkc` and `bynk` CLIs over
`bynk-driver`, the `bynkc-lsp` language server (`bynk-lsp`) over `bynk-ide`, and
the unpublished `bynk-wasm` playground module over `bynk-emit`, `bynk-strip`,
and `bynk-ide`.

Most users see this crate's output through the
[`bynkc`](https://crates.io/crates/bynkc) / [`bynk`](https://crates.io/crates/bynk)
CLIs (via [`bynk-driver`](https://crates.io/crates/bynk-driver)) rather than
depending on it directly.

## Use

```toml
[dependencies]
bynk-render = "0.390"
```

```rust
// `errors: &[bynk_syntax::CompileError]`, with the source and a label.
bynk_render::print_errors(errors, source, filename);          // ariadne, to stderr
let short = bynk_render::render_errors_short(errors, source, filename);
```

See the [API docs](https://docs.rs/bynk-render) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
