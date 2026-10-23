# bynk-ir

[![crates.io](https://img.shields.io/crates/v/bynk-ir.svg)](https://crates.io/crates/bynk-ir)
[![docs.rs](https://img.shields.io/docsrs/bynk-ir)](https://docs.rs/bynk-ir)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **resolved declaration-level vocabulary the [Bynk](https://github.com/bynk-lang/bynk)
emitter reads**, instead of re-deriving the same facts from
`bynk_syntax::ast` a second time.

Every type here is a value some [`bynk-lower`](https://crates.io/crates/bynk-lower)
helper returns from a syntax-tree node plus the checker's own typed
program — a service's protocol, a handler's kind and route cache, a `type`
declaration's structure, an agent store field's storage shape, a capability
op's signature, a `given` capability reference, an actor's authentication
seam, an events subscription's pattern and shape, and the literal values a
pattern can carry. Alongside them: four AST-walk helpers shared between
`bynk-lower` and `bynk-emit` (`block_uses_emit`, `walk_block_exprs`,
`walk_exprs`, `match_needs_if_chain`).

**Not an expression-level IR.** An earlier version of this crate held a full
typed expression IR — `IrExpr`, pattern/match compilation, every declaration
variant of `IrItem` — built but never consumed by the emitter; it was deleted
once a full pricing showed finishing that cutover would mean a second code
generator, not a retype (`design/greenfield-status.md`'s `unconsumed_ir_items`
probe holds this at 0). What's left is the declaration-level facts
`bynk-emit` genuinely reads today.

## Where it sits

This crate depends on [`bynk-syntax`](https://crates.io/crates/bynk-syntax)
and [`bynk-check`](https://crates.io/crates/bynk-check) (for `TypedCommons`
and the resolver's own types):

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

`bynk-lower` builds `bynk-ir` values from a checked program; `bynk-emit` is the
consumer both crates exist to serve. Every `pub` item here is required to have a
reader outside both `bynk-ir` and `bynk-lower` — a gated CI probe fails the
build otherwise.

## Use

```toml
[dependencies]
bynk-ir = "0.397"
```

`bynk-ir` values are constructed by `bynk-lower`'s own helpers, not built by
hand — see that crate's docs for the lowering entry points. This crate's own
surface is the value types and the shared AST-walk helpers.

See the [API docs](https://docs.rs/bynk-ir) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
