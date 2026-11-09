# bynk-model

[![crates.io](https://img.shields.io/crates/v/bynk-model.svg)](https://crates.io/crates/bynk-model)
[![docs.rs](https://img.shields.io/docsrs/bynk-model)](https://docs.rs/bynk-model)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **[Bynk](https://github.com/bynk-lang/bynk) system model**: the resolved
description of what a Bynk program *is* at the level of declarations. That
covers:
- a service's protocol, and a handler's kind, route and cache policy
- a `type` declaration's structure, and an agent store field's storage shape
- a capability op's or attached method's signature, and a `given` capability
  reference
- an actor's authentication seam, and an events subscription's pattern and
  shape
- a service's entry handlers' signatures, described structurally for a host
  such as the playground

Every tool that needs these facts reads them here rather than re-deriving them
from the syntax tree. Today those tools are the emitter, the editor's analysis
(`bynk-ide`), `bynk openapi`, the smoke runner and the playground.

It has three modules, all re-exported at the crate root:
- **`vocabulary`**: the model's items. This was the `bynk-ir` crate until the
  crate-hygiene track merged the two.
- **`lower`**: the builder from a checked program to those items. This was the
  `bynk-lower` crate.
- **`entry`**: the entry handlers' signatures. This was
  `bynk_check::entry_signature` until crate-hygiene B3.

This is distinct from `bynk_check::project_model`, which models units and
manifests, not declarations.

The model's boundary half, the wire contract (`bynk_check::wire::WireModel`),
is part of the system model but lives in `bynk-check`, because the checker's
own boundary checks read it. The model itself stays above `bynk-check`.

**What it is not.**
- **An expression IR.** Bodies stay with the emitter's own lowerer, and a model
  item may summarise a body but never represents one.
- **The emitted TypeScript's ABI.**
- **A second checker.** The model is built after checking and carries no
  diagnostics of its own.

## Where it sits

This crate depends on [`bynk-syntax`](https://crates.io/crates/bynk-syntax)
and [`bynk-check`](https://crates.io/crates/bynk-check) (for `TypedCommons`
and the resolver's own types):

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

Every `pub` item here must have a reader outside this crate, and a gated CI
probe (`unconsumed_ir_items`) fails the build otherwise. A second gated probe
(`model_readers_beyond_emit`) counts the items read outside the emitter, and
that count may only rise.

## Use

```toml
[dependencies]
bynk-model = "0.522"
```

```rust,ignore
use bynk_model::{IrHandlerKind, lower_handler_kind_ir};

match lower_handler_kind_ir(&handler.kind) {
    IrHandlerKind::Http { method, path } => println!("{} {path}", method.as_str()),
    _ => {}
}
```

See the [API docs](https://docs.rs/bynk-model) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
