# bynk-wasm

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **[Bynk](https://github.com/bynk-lang/bynk) compiler compiled to a wasm
module**, backing the in-browser [playground](https://github.com/bynk-lang/playground)
(served at [playground.bynk-lang.dev](https://playground.bynk-lang.dev)) and the
VS Code web extension.

> **Not a published crate.** `bynk-wasm` is `publish = false` — an internal build
> artefact, not on crates.io or docs.rs. It is built by `scripts/build-wasm.sh`,
> never via `cargo install`, and each release ships it in `bynk-web-<tag>.tar.gz`
> (`scripts/package-web-assets.sh`), which the playground builds from.

## What it does

A single entry point — `bynk_compile` (wasm) / `compile` (native) — takes
in-memory Bynk source and returns a runnable **JavaScript module graph** plus
diagnostics, with **no filesystem and no `tsc`**:

```text
source ─▶ bynk_emit::compile_in_memory  ─▶ ProjectOutput (TypeScript)
       ─▶ bynk_strip::strip_project_to_js ─▶ ProjectOutput (JavaScript)
       ─▶ { files: [{ path, kind, contents }], diagnostics }
```

The returned graph is the complete set the browser links: the user module,
`runtime.js`, the `bynk-browser.js` binding, and `compose.js`. Each file carries a `kind`: `"user"` (modules authored from the
source, e.g. `hello/world.js`), `"wiring"` (`compose.js`) or `"support"`
(`runtime.js`, `bynk-browser.js`, injected first-party units such as `bynk.js`),
so a consumer can show only the user's code while still linking the whole graph
(TS: `kind: "user" | "wiring" | "support"` on `EmittedFile`). Three companion
entries back the playground's editor: `bynk_analyze` returns diagnostics only,
for live on-type checking; `bynk_hover` returns the inferred type at an offset;
`bynk_complete` returns completion candidates at an offset; and `bynk_format`
returns the source in house style (`{ok, formatted}` / `{ok:false, error}`). Every entry
compiles for the `browser` platform.

The pipeline reuses the on-disk compile path wholesale (first-party injection,
the per-platform binding, the strip-only emitter), so the in-browser result
matches the CLI's.

## Build

The crate builds two ways:

- **`cdylib`** — the `wasm32` output in the release's browser artefacts.
- **`rlib`** — lets the native test build link the same logic, so the compile
  path is verified without a browser.

It is built by `scripts/build-wasm.sh` rather than installed on its own. Its
exports are the playground's interface: ci.yml's `web-assets` job checks the
tarball still exports what the playground calls.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
