# tree-sitter-bynk

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for the
[Bynk](https://github.com/bynk-lang/bynk) language — the source of truth for
tree-sitter syntax highlighting (`queries/highlights.scm`), the playground's
editor, and the Book's grammar reference (rendered from `src/grammar.json` by
`bynk-tests`'s `grammar` module, formerly the `bynk-grammar` crate, whose [last crates.io release](https://crates.io/crates/bynk-grammar) points here).

This directory is **two things**:

- **The grammar** (`grammar.js`), published to npm as
  [`tree-sitter-bynk`](https://www.npmjs.com/package/tree-sitter-bynk), with
  its generated Node binding (`bindings/node/`). This is what tree-sitter
  editors consume. (`vscode-bynk` ships its own TextMate grammar and does not
  use this one.)
- **A Rust crate** (this `Cargo.toml`, wrapping `bindings/rust/`) that exists
  for exactly one purpose: the cross-parser conformance test
  (`tests/conformance.rs`), which parses Bynk source with this grammar and
  diffs the accept/reject decision against the hand-written
  [`bynk-syntax`](https://crates.io/crates/bynk-syntax) recursive-descent
  parser — the only guard tying the editor grammar to the compiler's own.

> **Not a published Rust crate.** The Rust binding is `publish = false` and
> kept off the release workflows' crate list; only the grammar itself ships
> to npm.

The grammar deliberately stays **permissive** in the places where Bynk's type
checker would reject a program — semantic rules (type checking,
exhaustiveness, effect propagation, `given` matching) are left entirely to
the LSP (`bynk-lsp`), not encoded here.

## Building

```sh
npm install
npx tree-sitter generate
npx tree-sitter test
```

The Rust binding builds as an ordinary workspace member (`cargo build -p
tree-sitter-bynk`); `bynk-syntax` does not depend on it, so the conformance
test's dev-dependency edge is cycle-free.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
