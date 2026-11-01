# bynk-check

[![crates.io](https://img.shields.io/crates/v/bynk-check.svg)](https://crates.io/crates/bynk-check)
[![docs.rs](https://img.shields.io/docsrs/bynk-check)](https://docs.rs/bynk-check)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **semantic-analysis layer of the [Bynk](https://github.com/bynk-lang/bynk)
compiler** — name resolution and type checking over the
[`bynk-syntax`](https://crates.io/crates/bynk-syntax) AST.

It holds:

- `resolver` — builds the symbol table; flags duplicates, name overlap,
  unresolved references, and arity mismatches.
- `checker` — type-checks every declaration and expression, validates refinement
  predicates, and resolves capabilities, services, agents, and actors.
- `kernel_methods` / `builtin_names` — the registries the checker dispatches and
  the editor reads for `.`-member completion.
- `firstparty` — the first-party `bynk` surface, stdlib, and adapters,
  re-exported unchanged from [`bynk-std`](https://crates.io/crates/bynk-std)
  (including `Platform`).
- `actors` — actor-contract analysis (auth schemes, identities).
- `requirements` — the capability/requirement analysis the checker draws on.
- `analysis` / `project_model` / `check_pipeline` — whole-project analysis
  without emitting (`analysis::analyse_project`): discovery → parse → group →
  resolve → check, shared by the build and the IDE.
- `schema_registry` — reconciles event schemas against `bynk.schema.lock`.
- `wire` / `contract` — the wire-contract shape of a type crossing a context
  boundary, and the canonical form of a cross-context contract.
- `index` / `hints` / `expr_types` / `locals` — the **captured analysis tables**
  written during checking (the binding index, inlay hints, expression types,
  scoped locals) that the IDE layer queries.
- `agent_state` / `store_write` / `ast_text` — the agent state model: each
  agent's state machine derived from its handlers, with the `transition`
  overlay, shared by the editor's state view and the checker (#2220).

## Where it sits

`bynk-check` is the layer between the syntax leaf and everything that emits or
analyses; besides [`bynk-syntax`](https://crates.io/crates/bynk-syntax) it
depends only on [`bynk-project`](https://crates.io/crates/bynk-project) (the
project model), [`bynk-std`](https://crates.io/crates/bynk-std) (the
first-party sources, as data) and [`bynk-fmt`](https://crates.io/crates/bynk-fmt)
(whose expression printer labels the agent state model's guards):

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

The captured tables live here, with their producers; the IDE *queries* over them
live in [`bynk-ide`](https://crates.io/crates/bynk-ide). Most users compile
Bynk through the [`bynkc`](https://crates.io/crates/bynkc) /
[`bynk`](https://crates.io/crates/bynk) CLIs rather than depending on this crate
directly.

## Use

```toml
[dependencies]
bynk-check = "0.478"
```

```rust
use bynk_check::{checker, resolver};

let resolved = resolver::resolve(commons)?;
let typed = checker::check(resolved)?;
```

See the [API docs](https://docs.rs/bynk-check) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
