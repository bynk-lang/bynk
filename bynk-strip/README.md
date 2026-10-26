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
- `strip_project_to_js` — rewrite a compiled `bynk-emit` project to a JS
  artefact: each `.ts` module is stripped and renamed to `.js`, the
  `tsconfig.json` is dropped, and every other file passes through unchanged.
- `StripError` — a strip failure; for valid emitter output it should never
  occur, so it signals an emitter or toolchain bug rather than user error.

The engine is [`oxc`](https://crates.io/crates/oxc) — a pure-Rust TypeScript
parser, type-erasing transform, and codegen — so neither `bynkc --emit js` nor
the in-browser compile path pulls in Node or `tsc`, and the crate compiles to
`wasm32` for the playground. Stripping is configured for pure type-erasure
(matching Node's `stripTypeScriptTypes`): every *value* import is kept even when
unused, and only `import type` / `type` specifiers are elided.

## Where it sits

`bynk-strip` sits above [`bynk-emit`](https://crates.io/crates/bynk-emit),
turning its `ProjectOutput` into a JS artefact:

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

The dependency runs one way only — `bynk-emit` does not depend on `bynk-strip`,
and the language server's analysis layer (`bynk-ide`) depends on neither — so
`bynkc-lsp` never pulls in `oxc`.

## Use

```toml
[dependencies]
bynk-strip = "0.410"
```

```rust
let js = bynk_strip::strip_types("const n: number = 1;", "main.ts")?;
assert_eq!(js.trim(), "const n = 1;");
```

See the [API docs](https://docs.rs/bynk-strip) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
