# bynk-syntax

[![crates.io](https://img.shields.io/crates/v/bynk-syntax.svg)](https://crates.io/crates/bynk-syntax)
[![docs.rs](https://img.shields.io/docsrs/bynk-syntax)](https://docs.rs/bynk-syntax)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **syntax foundation of the [Bynk](https://github.com/bynk-lang/bynk)
compiler** — the lowest leaf of the compiler's layered crate set.

It holds the modules every other layer depends *on* and none depend *up* from:

- `lexer` — the [`logos`](https://crates.io/crates/logos)-driven token stream.
- `parser` / `ast` — hand-written recursive descent and the syntax tree it builds.
- `print` — renders an expression, pattern or statement back to Bynk source text (shared by the formatter and the checker's diagnostics).
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

Most users compile Bynk through the
[`bynk`](https://crates.io/crates/bynk) CLI rather than depending on this crate
directly; it is published so tooling that needs only to lex or parse Bynk can do
so without linking the whole compiler.

## Use

```toml
[dependencies]
bynk-syntax = "0.595"
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
