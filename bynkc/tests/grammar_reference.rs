//! Generates the complete-grammar appendix from `tree-sitter-bynk/src/grammar.json`
//! and keeps it up to date.
//!
//! `site/src/content/docs/book/reference/grammar-appendix.md` is rendered from
//! the compiled grammar (via the `bynk-grammar` crate), so it cannot drift from
//! the `tree-sitter-bynk` grammar. That grammar is a separate parser from the
//! compiler's own recursive-descent parser (`bynk-syntax`); the two are held in
//! agreement by the cross-parser conformance test
//! (`tree-sitter-bynk/tests/conformance.rs`), so the appendix describes what the
//! compiler accepts, not only what the editor grammar does. The annotated,
//! per-construct reference (`reference/grammar.md`) embeds the same generated
//! productions via the `{{#grammar}}` remark directive.
//! Regenerate the appendix after a grammar change with:
//!     BYNK_BLESS=1 cargo test -p bynkc --test it grammar_reference::

mod common;

use std::fs;
use std::path::PathBuf;

fn grammar_json() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tree-sitter-bynk/src/grammar.json");
    fs::read_to_string(path).expect("read grammar.json")
}

#[test]
fn generated_grammar_appendix_is_up_to_date() {
    let page = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../site/src/content/docs/book/reference/grammar-appendix.md");
    let rendered =
        common::to_site_page(&bynk_grammar::render_appendix(&grammar_json()), "reference");

    if std::env::var_os("BYNK_BLESS").is_some() {
        fs::write(&page, &rendered).unwrap();
        return;
    }

    let current = fs::read_to_string(&page).unwrap_or_default();
    assert_eq!(
        current, rendered,
        "site/src/content/docs/book/reference/grammar-appendix.md is out of date with the grammar.\n\
         Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it grammar_reference::"
    );
}
