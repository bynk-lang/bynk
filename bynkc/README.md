# bynkc

> **Final release: `bynkc` is now [`bynk`](https://crates.io/crates/bynk).** The
> separate compiler binary is retired: `bynk` runs every command in-process
> (`bynk check`, `fmt`, `compile` and `test` take the same flags and print the same
> output), and answers to the name `bynkc` during a short transition. This is `bynkc`'s
> final crates.io release (crate-hygiene track
> [#2775](https://github.com/bynk-lang/bynk/issues/2775), slice E3). It is not
> yanked, and existing pins keep resolving. New installs use `cargo install bynk`.

[![crates.io](https://img.shields.io/crates/v/bynkc.svg)](https://crates.io/crates/bynkc)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

This crate is a few-line shim that calls into `bynk`, so `cargo install bynkc`
still installs a working `bynkc` during that transition. It prints one line to stderr saying
the name is retired; nothing else about its output changes.

## What to change

| You ran | Run |
|---|---|
| `cargo install bynkc` | `cargo install bynk` |
| `bynkc check <input>` | `bynk check <input>` |
| `bynkc compile <input> -o <output>` | `bynk compile <input> -o <output>` (same `--target`, `--platform`, `--emit`) |
| `bynkc fmt …` | `bynk fmt …` |
| `bynkc test …` | `bynk test …` |
| `BYNK_BYNKC=…` to pin a compiler | nothing: it is ignored (there is no second compiler) |

The prebuilt release archives and the install scripts still carry a `bynkc` (a copy
of `bynk`) during the transition only; a later release drops it.

## The library is gone

Older `bynkc` releases also exposed a library (`bynkc::compile_project`,
`bynkc::write_output`, …). It was re-exports of the layered crates; depend on those
directly: [`bynk-driver`](https://crates.io/crates/bynk-driver) (project
orchestration, `write_output`, the command bodies),
[`bynk-emit`](https://crates.io/crates/bynk-emit) (`compile`),
[`bynk-render`](https://crates.io/crates/bynk-render) (diagnostic rendering),
[`bynk-check`](https://crates.io/crates/bynk-check) and
[`bynk-syntax`](https://crates.io/crates/bynk-syntax).

## License

Licensed under either of Apache License 2.0 or MIT, at your option.
