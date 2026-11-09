# bynk-strip

[![crates.io](https://img.shields.io/crates/v/bynk-strip.svg)](https://crates.io/crates/bynk-strip)
[![docs.rs](https://img.shields.io/docsrs/bynk-strip)](https://docs.rs/bynk-strip)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**Strip-only TypeScript → JavaScript for [Bynk](https://github.com/bynk-lang/bynk)'s
first-class JS artefact.** It erases TypeScript type syntax while preserving
every runtime construct and value import, so a JS artefact is simply the
emitter's TypeScript with its types deleted.

Because the Bynk emitter is **strip-only** — every emitted `.ts` is erasable by
pure type-stripping, never a type-directed lowering (no parameter properties,
`enum`s, or `namespace`s) — the transform here is total and lossless for runtime
behaviour: it only deletes type syntax, it never has to rewrite semantics.

- `strip_types` — strip one TypeScript source string to JavaScript.
- `strip_types_mapped` — the same, also composing the module's Bynk-to-JavaScript
  source map from the emitter's Bynk-to-TypeScript one.
- `StripError` — a strip failure; for valid emitter output it should never
  occur, so it signals an emitter or toolchain bug rather than user error.

The engine is [`oxc`](https://crates.io/crates/oxc) — a pure-Rust TypeScript
parser, type-erasing transform, and codegen — so neither `bynk compile --emit js` nor
the in-browser compile path pulls in Node or `tsc`, and the crate compiles to
`wasm32` for the playground. Stripping is configured for pure type-erasure
(matching Node's `stripTypeScriptTypes`): every *value* import is kept even when
unused, and only `import type` / `type` specifiers are elided.

## Where it sits

`bynk-strip` is a leaf: it depends on `oxc` alone, so it builds alongside the checker.
[`bynk-driver`](https://crates.io/crates/bynk-driver) calls it to turn a compiled
project into a JS artefact (`bynk_driver::strip::strip_project_to_js`: each `.ts` module
is stripped and renamed to `.js`, the `tsconfig.json` is dropped, and every other file
passes through unchanged):

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

The dependency runs one way only — `bynk-strip` depends on no other Bynk crate, and the
language server's analysis layer (`bynk-ide`) does not depend on it — so `bynk-lsp` never
pulls in `oxc`.

## Use

```toml
[dependencies]
bynk-strip = "0.522"
```

```rust
let js = bynk_strip::strip_types("const n: number = 1;", "main.ts")?;
assert_eq!(js.trim(), "const n = 1;");
```

See the [API docs](https://docs.rs/bynk-strip) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
