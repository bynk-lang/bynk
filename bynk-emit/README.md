# bynk-emit

[![crates.io](https://img.shields.io/crates/v/bynk-emit.svg)](https://crates.io/crates/bynk-emit)
[![docs.rs](https://img.shields.io/docsrs/bynk-emit)](https://docs.rs/bynk-emit)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**Build orchestration and TypeScript emission for the
[Bynk](https://github.com/bynk-lang/bynk) compiler.**

It is the layer above [`bynk-check`](https://crates.io/crates/bynk-check) that
turns a checked program into output:

- `project` — build orchestration: the `compile_project` / `check_project`
  entry points, per-unit build sequencing, and `compile_in_memory` (the
  filesystem-free path behind the playground). Discovery, the dependency graph,
  and path resolution live in [`bynk-project`](https://crates.io/crates/bynk-project)
  and are re-exported here; whole-project analysis without building lives in
  [`bynk-check`](https://crates.io/crates/bynk-check)'s own
  `analysis::analyse_project`.
- `emitter` — lowers a type-checked program to TypeScript targeting Cloudflare
  Workers (or a single bundle), complete with the router, dependency wiring, the
  shared runtime, and a `wrangler.toml`.

The `workers` target emits one Worker per context; the `compile_project` result
is an in-memory tree of TypeScript files — writing it to disk is the driver's
job (`bynk-driver::write_output`), not this crate's.

## Where it sits

`bynk-emit` turns a checked program into output, over
[`bynk-check`](https://crates.io/crates/bynk-check),
[`bynk-project`](https://crates.io/crates/bynk-project), the
[`bynk-model`](https://crates.io/crates/bynk-model) (the system model and its
builder), and
[`bynk-ts`](https://crates.io/crates/bynk-ts)'s tree:

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
directly.

## Use

```toml
[dependencies]
bynk-emit = "0.531"
```

```rust
use bynk_driver::project::{compile_project, CompileOptions, BuildTarget};

// `sources` maps every `.bynk` file under `root` (by canonical path) to its text.
let options = CompileOptions::single(root).sources(sources).target(BuildTarget::Workers);
let output = compile_project(&options)?; // in-memory TypeScript tree — bynk-emit never touches disk
```

Sources are supplied in memory because this crate never reads them from disk;
[`bynk-driver`](https://crates.io/crates/bynk-driver)'s `project_options` reads
a project from disk and returns options ready to compile.

See the [API docs](https://docs.rs/bynk-emit) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
