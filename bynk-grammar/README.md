# bynk-grammar

> **No longer published after this release.** `bynk-grammar` only renders the
> grammar reference for the Bynk repository's own tests, so no installed binary
> depends on it. This is its final crates.io release; it is not yanked, and
> existing pins keep resolving. It continues, unpublished, at
> [`bynk-lang/bynk`](https://github.com/bynk-lang/bynk/tree/main/bynk-grammar).
> Nothing replaces it as a library: the rendered grammar is the Book's
> [grammar reference](https://github.com/bynk-lang/bynk/tree/main/site/src/content/docs/book/reference).


[![crates.io](https://img.shields.io/crates/v/bynk-grammar.svg)](https://crates.io/crates/bynk-grammar)
[![docs.rs](https://img.shields.io/docsrs/bynk-grammar)](https://docs.rs/bynk-grammar)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Renders the [`tree-sitter-bynk`](https://github.com/bynk-lang/bynk/tree/main/tree-sitter-bynk)
grammar to **EBNF** for the [Bynk](https://github.com/bynk-lang/bynk) language
reference.

This crate is the single source of the grammar reference. It takes the compiled
grammar JSON (`tree-sitter-bynk/src/grammar.json`) as input and is otherwise
location-agnostic, so the same renderer feeds both:

- the full grammar **appendix** page (`render_appendix`), and
- the per-rule **includes** embedded in the curated reference pages
  (`render_production`, collected for the site by `render_site_json`).

Because both come from one implementation, an embedded production cannot drift
from the appendix.

## Readable names

Grammar rule names are parser-internal (`_type_ref`, `_expression`, …). For the
reference, rules are rendered under *readable* names via `display_name`: a
trivial `_x ::= y` wrapper collapses to its target, an optional override applies,
otherwise a single leading underscore is stripped. The transform is applied to
both rule heads and the nonterminal references inside productions, so the whole
reference reads as language, not internals.

## Consumers

This is an internal build-time crate for the Bynk project — it has no runtime
purpose in a Bynk program. Its consumers are:

- `bynkc/tests/grammar_reference.rs` — generates the appendix page, blessed
  against the committed `site/src/content/docs/book/reference/grammar.md`.
- `bynk-grammar/tests/generated_grammar_json.rs` — generates
  `site/src/generated/grammar.json` (`render_site_json`), blessed the same way.
- `site/src/plugins/remark-bynk-directives.mjs` — the `{{#grammar <rule>}}`
  include directive the Book renders with, reading that generated JSON.

## Use

```rust
use bynk_grammar::{render_appendix, render_rule};

let grammar_json = std::fs::read_to_string("tree-sitter-bynk/src/grammar.json")?;

// The full appendix, infallible over a well-formed grammar.
let ebnf = render_appendix(&grammar_json);

// A single rule, by its grammar name (`_expression` renders as `expression`)
// — `Err(GrammarError)` if it is unknown.
let one = render_rule(&grammar_json, "_expression")?;
```

`render_rule`, `render_production`, and `display_name` take the rule's grammar
name, not its readable one, and return `Result<_, GrammarError>` (unparseable
JSON, or an unknown rule name). See the
[API docs](https://docs.rs/bynk-grammar).

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
