//! Drift guard for the documentation site's grammar-semantics artifact.
//!
//! `site/src/generated/grammar-semantics.json` is the committed map from grammar
//! rule to the diagnostics that constrain it, which the Astro/Starlight
//! `{{#grammar-semantics}}` remark directive reads (documentation track, slice 2).
//! It is the same render as the mdBook-facing `docs/grammar-semantics.json`
//! ([`bynk_syntax::diagnostics::render_grammar_semantics_json`]) — the site keeps its
//! own committed copy so it does not read from `docs/` (which retires later). This
//! test regenerates it and fails if the committed copy is stale.
//!
//! Regenerate with:
//!   BYNK_BLESS=1 cargo test -p bynkc --test it generated_site_json::

use std::fs;
use std::path::PathBuf;

use bynk_syntax::diagnostics::render_grammar_semantics_json;

#[test]
fn generated_site_grammar_semantics_json_is_up_to_date() {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../site/src/generated/grammar-semantics.json");
    let rendered = render_grammar_semantics_json();

    if std::env::var_os("BYNK_BLESS").is_some() {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, &rendered).unwrap();
        return;
    }

    let current = fs::read_to_string(&file).unwrap_or_default();
    assert_eq!(
        current, rendered,
        "site/src/generated/grammar-semantics.json is out of date with the registry.\n\
         Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it generated_site_json::"
    );
}
