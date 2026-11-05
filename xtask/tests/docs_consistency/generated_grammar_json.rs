//! Drift guard for the documentation site's grammar artifact.
//!
//! `site/src/generated/grammar.json` is the committed render of the grammar that
//! the Astro/Starlight `{{#grammar}}` remark directive embeds (documentation
//! track, slice 2). It comes from the same renderer as the mdBook preprocessor
//! ([`xtask::grammar::render_site_json`]), so the site cannot drift from the book.
//! This test regenerates it and fails if the committed copy is stale.
//!
//! Regenerate with:
//!   BYNK_BLESS=1 cargo test -p xtask --test docs_consistency generated_grammar_json::

use std::fs;
use std::path::PathBuf;

#[test]
fn generated_grammar_json_is_up_to_date() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let grammar = fs::read_to_string(root.join("tree-sitter-bynk/src/grammar.json"))
        .expect("read tree-sitter-bynk/src/grammar.json");
    let rendered = xtask::grammar::render_site_json(&grammar);

    let file = root.join("site/src/generated/grammar.json");
    if std::env::var_os("BYNK_BLESS").is_some() {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, &rendered).unwrap();
        return;
    }

    let current = fs::read_to_string(&file).unwrap_or_default();
    assert_eq!(
        current, rendered,
        "site/src/generated/grammar.json is out of date with the grammar.\n\
         Regenerate with: BYNK_BLESS=1 cargo test -p xtask --test docs_consistency generated_grammar_json::"
    );
}
