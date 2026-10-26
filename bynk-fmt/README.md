# bynk-fmt

[![crates.io](https://img.shields.io/crates/v/bynk-fmt.svg)](https://crates.io/crates/bynk-fmt)
[![docs.rs](https://img.shields.io/docsrs/bynk-fmt)](https://docs.rs/bynk-fmt)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **formatter for the [Bynk](https://github.com/bynk-lang/bynk) language**.
Given Bynk source, it produces the one canonical formatting — comments and
layout-significant trivia preserved.

Formatting is fundamentally an AST walk, so this crate depends on the
[`bynk-syntax`](https://crates.io/crates/bynk-syntax) leaf **only** — not the
type checker or emitter. That keeps it small: downstream consumers (the
[`bynkc-lsp`](https://crates.io/crates/bynk-lsp) language server and third-party
tools) get the formatter without pulling in the whole compiler. The
[`bynkc`](https://crates.io/crates/bynkc) compiler's `bynkc fmt` command
reaches it via `bynk-driver`, not a `bynkc::fmt` re-export.

Most users format Bynk through the CLI (`bynkc fmt`) or format-on-save in the
editor, rather than depending on this crate directly. See
[Format your code with `bynk-fmt`](https://bynk-lang.org/docs/editor-and-tooling/format/).

## Where it sits

`bynk-fmt` sits directly on the `bynk-syntax` leaf, alongside the other
first-layer libraries:

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

[`bynk-driver`](https://crates.io/crates/bynk-driver) (behind `bynkc fmt` /
`bynk fmt`) and [`bynk-lsp`](https://crates.io/crates/bynk-lsp) format through
it; [`bynk-ide`](https://crates.io/crates/bynk-ide) uses its expression and
annotation printers to render types and signatures for hover.

## Use

```toml
[dependencies]
bynk-fmt = "0.406"
```

```rust
use bynk_fmt::{format_source, FormatOptions};

let pretty = format_source(source, &FormatOptions::default())?;
```

The public API is small:

- `format_source(source, options) -> Result<String, FormatError>`
- `FormatOptions` / `IndentStyle` — formatting configuration.
- `FormatError` — a parse error in the input (you cannot format what does not
  parse).
- `FmtConfig` / `find_manifest` / `ConfigError` — a project's `[fmt]` section in
  `bynk.toml`, applied over a base `FormatOptions`.

`FormatOptions`'s three fields are reachable from a project's `bynk.toml` and
from the command line — `bynkc fmt` / `bynk fmt` take `--indent tab|spaces`,
`--indent-width N`, `--max-line-width COLUMNS`, and `--trailing-comma` /
`--no-trailing-comma`, overriding the project's `[fmt]` section for one run
(`--no-config` ignores that section entirely).

See the [API docs](https://docs.rs/bynk-fmt) for details.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
