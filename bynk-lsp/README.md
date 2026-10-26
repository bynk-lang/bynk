# bynk-lsp

[![crates.io](https://img.shields.io/crates/v/bynk-lsp.svg)](https://crates.io/crates/bynk-lsp)
[![docs.rs](https://img.shields.io/docsrs/bynk-lsp)](https://docs.rs/bynk-lsp)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **Language Server for the [Bynk](https://github.com/bynk-lang/bynk)
language**. The crate builds the `bynkc-lsp` binary, which any LSP-capable
editor can speak to for live diagnostics, navigation, and refactoring of `.bynk`
projects.

Built on [`tower-lsp`](https://crates.io/crates/tower-lsp), it links the
compiler's analysis libraries directly — [`bynk-ide`](https://crates.io/crates/bynk-ide)
(diagnostics + project analysis), [`bynk-check`](https://crates.io/crates/bynk-check),
[`bynk-syntax`](https://crates.io/crates/bynk-syntax), and the
[`bynk-fmt`](https://crates.io/crates/bynk-fmt) formatter — not the whole
compiler binary. So the editor sees exactly the diagnostics `bynkc check` would
report, without linking the CLI it doesn't need.

## Capabilities

- **Diagnostics** — re-runs the compiler on change and publishes errors and
  warnings with their dotted categories.
- **Hover** — type signatures and doc blocks.
- **Go-to-definition**, **go-to-type-definition**, **go-to-implementation**, and
  **find references** for types, functions, capabilities, services, and agents.
- **Call hierarchy**.
- **Rename** (workspace-wide, validated).
- **Formatting** and **range formatting** (via `bynk-fmt`).
- **Document & workspace symbols**, **document highlights**, **document links**,
  **folding ranges**, and **selection ranges**.
- **Completion**, **signature help**, **inlay hints** (inferred types and
  parameter names), **inline values** while debugging, **semantic tokens**
  (type-aware highlighting), **code lenses**, and **code actions** (quick fixes for suggested diagnostics).
- **Project views** — custom `bynk/architectureModel`, `bynk/sequenceModel`,
  `bynk/documentationModel`, and `bynk/wireContract` requests behind the VS Code
  extension's diagrams and peeks.
- **Diagnostic explanations** — a custom `bynk/explain` request serving the
  compiler's own explanation of a diagnostic code offline, behind an
  **Explain** quick fix for clients that opt in.
- **File watching** across the project.

The full capability list is specified in
[`design/bynk-lsp-spec.md`](https://github.com/bynk-lang/bynk/blob/main/design/bynk-lsp-spec.md).

## Install

```sh
cargo install bynk-lsp
```

Or build from the workspace:

```sh
cargo build --release -p bynk-lsp   # → target/release/bynkc-lsp
```

This produces the **`bynkc-lsp`** binary. Requires a stable Rust toolchain,
2024 edition (MSRV 1.95).

## Use

`bynkc-lsp` speaks LSP over stdio. Most users consume it through the
[VS Code extension](https://github.com/bynk-lang/bynk/tree/main/vscode-bynk),
which bundles and launches it automatically. For other editors, point your LSP
client at the `bynkc-lsp` binary and associate it with the `bynk` language /
`.bynk` files. See the
[`bynk-lsp` tooling docs](https://bynk-lang.org/docs/tooling/bynk-lsp/) and
[Set up editor support](https://bynk-lang.org/docs/editor-and-tooling/editor-support/).

The server discovers a project by walking up to the nearest `bynk.toml` (falling
back to single-file mode if there is none). It logs to `~/.bynk-lsp.log` at
`warn` by default; set `BYNK_LSP_LOG` (e.g. `BYNK_LSP_LOG=debug`) to raise the
level. `bynkc-lsp --version` prints the version.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
