# bynk-ts

[![crates.io](https://img.shields.io/crates/v/bynk-ts.svg)](https://crates.io/crates/bynk-ts)
[![docs.rs](https://img.shields.io/docsrs/bynk-ts)](https://docs.rs/bynk-ts)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **TypeScript tree and printer for the [Bynk](https://github.com/bynk-lang/bynk)
compiler**.

Emission builds a typed tree — `TsProgram` / `TsStmt` / `TsExpr` / `TsType` /
`TsDecl` — instead of writing TypeScript text by hand; this crate's printer is
the **only** code in the compiler that writes a character of emitted output.
A `Verbatim`/`VerbatimExpr` escape hatch, tagged by `VerbatimOrigin`, carries
content the tree doesn't yet represent structurally (a not-yet-converted
emitter fragment, a vendored `.ts` file staged verbatim) without losing track
of it: `verbatim_violations` scans that tagged content for constructs the
tree's own erasure guarantee depends on never containing (`enum`, `namespace`,
a decorator, a constructor parameter property, `any`).

It holds:

- `program` — the tree types themselves, and `TsProgram::verbatim_content`, a
  walker collecting every `Verbatim`/`VerbatimExpr` leaf's text.
- `printer` — the single writer; produces final TypeScript text plus a source
  map.
- `lint` — `verbatim_violations`, the textual scan over escape-hatch content.
- `source_map` — the source-map builder the printer threads through.

## Where it sits

`bynk-ts` depends on [`bynk-syntax`](https://crates.io/crates/bynk-syntax)
only — for `Span`, reused unchanged rather than redefined:

```text
bynk-syntax                lexer, parser, AST, CompileError, diagnostic codes
├── bynk-project           project model: discovery, unit graph, paths
├── bynk-ts                the TypeScript tree and its printer
├── bynk-render            diagnostic rendering
├── bynk-fmt               the formatter
└── bynk-check             name resolution and type checking   + project, std
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

It has no visibility into the checker, the IR, or any emitter-internal type; a
function taking one wouldn't compile. [`bynk-emit`](https://crates.io/crates/bynk-emit)
builds the tree this crate defines, and the printer is reached three ways:
[`bynk-driver`](https://crates.io/crates/bynk-driver) calls it at the real
filesystem write boundary, [`bynk-strip`](https://crates.io/crates/bynk-strip)
calls it to feed a `TsProgram` through `strip_types` on the way to a JS
artefact, and `Document::text()` calls it in-process for every other reader
that just needs bytes (golden fixtures, `bynk-wasm`'s JS-facing API).

## Use

```toml
[dependencies]
bynk-ts = "0.371"
```

```rust
let mut program = bynk_ts::TsProgram::new();
program.push(bynk_ts::TsStmt::const_stmt(
    bynk_ts::TsBindingName::Ident("answer".to_string()),
    None,
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("42".to_string())),
    None,
));
let printed = bynk_ts::print(&program, "", "", "");
assert_eq!(printed.text, "const answer = 42;\n");
```

See the [API docs](https://docs.rs/bynk-ts) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
