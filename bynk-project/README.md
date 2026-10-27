# bynk-project

[![crates.io](https://img.shields.io/crates/v/bynk-project.svg)](https://crates.io/crates/bynk-project)
[![docs.rs](https://img.shields.io/docsrs/bynk-project)](https://docs.rs/bynk-project)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

[Bynk](https://github.com/bynk-lang/bynk)'s **project model**: discovery, the
unit dependency graph, path resolution, cross-unit consistency checks, and
the schema-registry document's read/write halves.

It holds:

- `discovery` — walks a project's roots and finds every `.bynk` file (and, in
  project form, every unit's own manifest).
- `graph` — the unit dependency graph and its cycle/consistency checks.
- `paths` / `roots` — path resolution and the include/exclude root rules a
  project's layout is interpreted through.
- `consistency` — cross-unit checks (directory-kind, directory-name, group-
  kind, path-name alignment).
- `schema_registry` — reads and writes `bynk.schema.lock`, the schema
  registry document.
- `parse_cache` / `json` — supporting caches and JSON helpers the above use.

What deliberately stays **out** of this crate: schema-registry
*reconciliation* (checker-coupled, lives in `bynk-check`), diagnostic
modes and project-analysis result types (facts about how the pipeline is
driven, not about the project itself), and test codegen — all downstream
concerns, not project modelling.

## Where it sits

`bynk-project` sits directly on the `bynk-syntax` leaf (plus `toml`/`serde` for
the manifest and lock-file formats) — it has no dependency on the checker or the
emitter:

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

[`bynk-check`](https://crates.io/crates/bynk-check),
[`bynk-emit`](https://crates.io/crates/bynk-emit), and
[`bynk-ide`](https://crates.io/crates/bynk-ide) each depend on it directly for
discovery and path resolution, rather than re-deriving their own.

## Use

```toml
[dependencies]
bynk-project = "0.426"
```

```rust
use bynk_project::roots::Roots;

let roots = Roots::Single(std::path::PathBuf::from("src"));
```

See the [API docs](https://docs.rs/bynk-project) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
