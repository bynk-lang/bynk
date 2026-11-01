# bynk-ide

[![crates.io](https://img.shields.io/crates/v/bynk-ide.svg)](https://crates.io/crates/bynk-ide)
[![docs.rs](https://img.shields.io/docsrs/bynk-ide)](https://docs.rs/bynk-ide)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **IDE/LSP analysis surface for the [Bynk](https://github.com/bynk-lang/bynk)
compiler** — the non-bailing diagnostics a language server consumes.

Where the CLI compile path *bails* on the first failure and emits, this layer
analyses a whole project (or a single file) **without bailing**, returning every
diagnostic plus the captured analysis tables for the editor to query:

- `diagnose` — best-effort single-file diagnostics (lex → parse-with-recovery →
  resolve → check), always returning a diagnostic list.
- `diagnose_project` — overlay-aware, file-attributed whole-project analysis: the
  per-file diagnostics, the binding index, inlay hints, expression types, scoped
  locals, and the unit→source map.
- `discover_files` / `AnalysisRoots` — the project's file set, resolved exactly
  as the compiler resolves it (overlay-aware).

Over those results sit the editor queries: `completion`, `signature_help`,
`symbols` (hover and go-to-definition lookups), `locals_nav`, and the
whole-project models behind the editor's views — `architecture`, `sequence`,
`agent_effects`, `documentation`, and `wire_contract`. The agent state model
lives in `bynk-check`, shared with the checker, and is re-exported here as
`agent_state`.

## Where it sits

`bynk-ide` sits *beside* the emitter, not above it: it depends on
[`bynk-syntax`](https://crates.io/crates/bynk-syntax),
[`bynk-check`](https://crates.io/crates/bynk-check),
[`bynk-project`](https://crates.io/crates/bynk-project), and
[`bynk-fmt`](https://crates.io/crates/bynk-fmt), and never links `bynk-emit`:

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

The [`bynk-lsp`](https://crates.io/crates/bynk-lsp) language server is built on
it, so the editor links the analysis libraries — not the emitter or a CLI. The
playground's `bynk-wasm` module uses it too, for completion.

## Use

```toml
[dependencies]
bynk-ide = "0.463"
```

```rust
use std::collections::HashMap;

let single = bynk_ide::diagnose(source);                 // Vec<Diagnostic>
let project = bynk_ide::diagnose_project(root, &HashMap::new());
for file in &project.files {
    // file.source_path, file.diagnostics, …
}
```

Most users get these diagnostics through an editor (via
[`bynk-lsp`](https://crates.io/crates/bynk-lsp)) rather than depending on this
crate directly. See the [API docs](https://docs.rs/bynk-ide).

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
