# xtask

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Repo automation for the [Bynk](https://github.com/bynk-lang/bynk) compiler,
run as `cargo xtask <command>`.

> **Not a published crate.** `xtask` is `publish = false` — internal repo
> tooling, kept off the release workflows' crate list.

## Commands

- `check-pending` — validates the increment-declaration files under
  `design/pending/` (the increment-allocation track: a feature PR declares
  its version-bump intent in one file there; a merge-time stamp assigns the
  real numbers in merge order, so parallel PRs never race for the same
  version).
- `stamp` — assigns the version(s) and ADR number(s) for the pending files
  and materialises them. Dry-run by default; `--apply` to write.
- `greenfield-status` — runs the compiler-architecture probes
  (`design/greenfield-status.md`) against the tree and prints the report,
  exiting non-zero if a gated probe disagrees with the committed table — the
  same diff CI runs (`greenfield_status_table_is_current`). `--apply` writes the
  committed table.
- `ci` — runs the same gates CI runs, locally, in CI's own cheapest-first
  order; `--fast` runs formatting, clippy and the strict `cargo doc` (broken
  intra-doc links) but skips the test suite.

## Use

```sh
cargo xtask check-pending
cargo xtask greenfield-status
cargo xtask ci --fast
```

See `src/main.rs` for the full command list and each command's own doc
comment for what it checks.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
