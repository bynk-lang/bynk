# bynk-syntax

[![crates.io](https://img.shields.io/crates/v/bynk-syntax.svg)](https://crates.io/crates/bynk-syntax)
[![docs.rs](https://img.shields.io/docsrs/bynk-syntax)](https://docs.rs/bynk-syntax)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **syntax foundation of the [Bynk](https://github.com/bynk-lang/bynk)
compiler** — the lowest leaf of the compiler's layered crate set.

It holds the modules every other layer depends *on* and none depend *up* from:

- `lexer` — the [`logos`](https://crates.io/crates/logos)-driven token stream.
- `parser` / `ast` — hand-written recursive descent and the syntax tree it builds.
- `span` — source byte ranges, plus the `line_col` position utility.
- `keywords` — the reserved-word table.
- `error` — `CompileError` (the structured, spanned diagnostic every phase
  produces) and `Severity`.
- `diagnostics` — the registry of `bynk.*` diagnostic codes (the single source of
  truth for the codes, summaries, and grammar links).

Because diagnostics, positions, and codes all live here, they cross every crate
in the compiler without an upward dependency.

## Where it sits

`bynk-syntax` is the leaf of the layered compiler:

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

Most users compile Bynk through the [`bynkc`](https://crates.io/crates/bynkc) or
[`bynk`](https://crates.io/crates/bynk) CLIs rather than depending on this crate
directly; it is published so tooling that needs only to lex or parse Bynk can do
so without linking the whole compiler.

## Use

```toml
[dependencies]
bynk-syntax = "0.427"
```

```rust
use bynk_syntax::{lexer, parser};

let tokens = lexer::tokenize(source)?;
let unit = parser::parse(&tokens, source)?;
```

See the [API docs](https://docs.rs/bynk-syntax) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
