# bynk-lower

[![crates.io](https://img.shields.io/crates/v/bynk-lower.svg)](https://crates.io/crates/bynk-lower)
[![docs.rs](https://img.shields.io/docsrs/bynk-lower)](https://docs.rs/bynk-lower)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **AST-analysis helpers the [Bynk](https://github.com/bynk-lang/bynk) emitter
reads resolved declaration-level facts through** — handler kinds and `given`
clauses, service protocols, store-field shapes, capability op and
attached-method signatures, route cache/limit annotations, event-subscriber
shapes, and the store-write walk behind an agent's implicit-commit decision.

Each helper takes a checked program (or its `TypedCommons`) and a
syntax-tree node, and returns a [`bynk-ir`](https://crates.io/crates/bynk-ir)
value — none of them lowers a whole expression body; that machinery existed
here once and was deleted once it proved unreachable from production (see
`bynk-ir`'s own README for the full story).

## Where it sits

This crate depends on [`bynk-syntax`](https://crates.io/crates/bynk-syntax),
[`bynk-check`](https://crates.io/crates/bynk-check), and
[`bynk-ir`](https://crates.io/crates/bynk-ir):

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

[`bynk-emit`](https://crates.io/crates/bynk-emit) is the one production
consumer of every helper here — each has a real call site in the emitter, not
just a test. The `bynk-project` dev-dependency is test-fixture plumbing only
(building checked programs to lower in this crate's own tests), not a
production edge.

## Use

```toml
[dependencies]
bynk-lower = "0.411"
```

```rust
// `kind: &bynk_syntax::ast::HandlerKind`
let ir_kind = bynk_lower::lower_handler_kind_ir(kind);
```

See the [API docs](https://docs.rs/bynk-lower) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
