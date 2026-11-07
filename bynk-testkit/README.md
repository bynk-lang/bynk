# bynk-testkit

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Cross-crate test fixtures for the [Bynk](https://github.com/bynk-lang/bynk)
compiler, built directly on `bynk-ide`'s production discovery
(`bynk_ide::discover_files`).

> **Not a published crate.** `bynk-testkit` is `publish = false` — dev-only,
> and kept off the release workflows' crate list.

## What it does

Every helper here walks a project exactly the way production code already
does — `bynk_ide::discover_files` for `diagnose_project`-style callers, the
same `bynk_driver::project::Roots` a `CompileOptions` will itself compile for
`compile_options_single`/`compile_options_split` — and reads every file into
a complete sources map, instead of a test reimplementing the walk. There is
no second resolution to drift from the first: a test built on these helpers
cannot silently miss a file because this crate's notion of "the project's
files" diverged from the compiler's own.

```rust
// In an integration test:
let opts = bynk_testkit::compile_options_single(src_dir);
let output = bynk_driver::project::compile_project(&opts)?;
```

This crate ships no production code, and is invisible to
`design/greenfield-status.md`'s `fs_below_driver` probe, which is scoped to the
crates below the driver (`bynk-emit`, `bynk-ide`, `bynk-fmt`) — not to
dev-only crates like this one.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
