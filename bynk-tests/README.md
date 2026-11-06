# bynk-tests

The Bynk integration-test corpus.

> **Not a published crate.** `bynk-tests` is `publish = false` and kept off the
> release workflows' crate list.

## The corpus

`tests/` is the compiler's integration suite: the `fixtures/` and `golden/`
trees and the behaviour suites. (The docs and index consistency checks, and the
grammar-reference renderer, are in `xtask/tests/docs_consistency`.) It is **one test binary**, `it` (see `tests/it.rs`), so cargo links the
compiler once rather than once per file. Run a file's tests by its module name:

```sh
cargo test -p bynk-tests --test it behaviour_fixtures::
cargo nextest run -p bynk-tests --test it -E 'test(/^system::tsc_verify::/)'
```

## Tiers

The files are grouped by tier: `tests/integration/` (in-process pipeline tests
and the fixture corpus) and `tests/system/` (anything that starts the compiler
binary, `tsc`, `node`, `workerd` or `wrangler`). A test's name starts with its
tier, `system::tsc_verify::…`, and `.config/nextest.toml`'s `ci-unit`,
`ci-integration` and `ci-system` profiles select by it. `cargo xtask tier-probe`
fails when a test is in no tier or in two. In CI the Linux `test` job runs unit +
integration (`ci-lower`) and the parallel `test-system` job runs `ci-system`; locally,
`cargo nextest run --profile ci-system` (or `ci-lower`) runs one half. See `tests/it.rs`.

Regenerate a blessed reference page with `BYNK_BLESS=1`, for example
`BYNK_BLESS=1 cargo test -p xtask --test docs_consistency grammar_reference::`.

Tests that start the compiler binary use `CARGO_BIN_EXE_bynk-under-test`, a
forwarding binary in this crate that `include!`s `bynk`'s own `main.rs`
(Cargo only sets `CARGO_BIN_EXE_*` for a package's own binaries). They drive
`bynk test`, `check`, `compile` and `fmt`; the separate `bynkc` binary they
once drove is retired (track E3).

## The grammar renderer

`bynk_tests::grammar` renders the `tree-sitter-bynk` grammar
(`tree-sitter-bynk/src/grammar.json`) to EBNF for the language reference: the
full appendix page (`render_appendix`) and the per-rule includes embedded in the
curated pages (`render_production`, collected for the site by
`render_site_json`). It was the `bynk-grammar` crate until that crate stopped
being published and folded in here.

## Layout

The directory structure is kept flat for now. Grouping the modules by test tier
is a later step of the crate-hygiene track.
