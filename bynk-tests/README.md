# bynk-tests

The Bynk integration-test corpus and the grammar-reference renderer.

> **Not a published crate.** `bynk-tests` is `publish = false` and kept off the
> release workflows' crate list.

## The corpus

`tests/` is the compiler's integration suite: the `fixtures/` and `golden/`
trees, the behaviour suites, and the drift guards for the generated reference
pages. It is **one test binary**, `it` (see `tests/it.rs`), so cargo links the
compiler once rather than once per file. Run a file's tests by its module name:

```sh
cargo test -p bynk-tests --test it behaviour_fixtures::
cargo nextest run -p bynk-tests --test it -E 'test(/^tsc_verify::/)'
```

Regenerate a blessed reference page with `BYNK_BLESS=1`, for example
`BYNK_BLESS=1 cargo test -p bynk-tests --test it grammar_reference::`.

Tests that start the compiler binary use `CARGO_BIN_EXE_bynkc-under-test`, a
forwarding binary in this crate that `include!`s `bynkc`'s own `main.rs`
(Cargo only sets `CARGO_BIN_EXE_*` for a package's own binaries). It goes away
when `bynkc` becomes an alias of `bynk`.

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
