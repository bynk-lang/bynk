//! Keeps the keyword registry, the lexer, and the generated reference page in
//! lock-step.
//!
//! 1. The alphabetic `#[token("…")]` keywords in `lexer.rs` must match exactly
//!    `bynk_syntax::keywords::KEYWORDS`.
//! 2. `site/src/content/docs/book/reference/keywords.md` must match what the
//!    registry renders.
//! 3. The LLM primer's hand-written `## Keywords` section (`site/public/llms.txt`)
//!    must list exactly the registry's words (#2464).
//!
//! Regenerate the docs page with:
//!     BYNK_BLESS=1 cargo test -p bynkc --test it keywords_reference::

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use bynk_syntax::keywords::{
    BUILTIN_TYPE_NAMES, CONTEXTUAL_KEYWORDS, KEYWORDS, RESERVED_CONTEXTUAL, render_markdown,
};

/// Alphabetic keyword tokens declared in the lexer via `#[token("…")]`. The
/// lexer now lives in the `bynk-syntax` leaf (crate-decomposition slice 1), so
/// this reads across the crate boundary.
fn keywords_in_lexer() -> BTreeSet<String> {
    let lexer = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../bynk-syntax/src/lexer.rs");
    let text = fs::read_to_string(&lexer).unwrap();
    let re = regex::Regex::new(r#"#\[token\("([a-zA-Z][a-zA-Z_]*)"\)\]"#).unwrap();
    re.captures_iter(&text).map(|c| c[1].to_string()).collect()
}

fn registry_keywords() -> BTreeSet<String> {
    KEYWORDS.iter().map(|k| k.word.to_string()).collect()
}

#[test]
fn registry_is_sorted_with_no_duplicates() {
    let words: Vec<&str> = KEYWORDS.iter().map(|k| k.word).collect();
    let mut sorted = words.clone();
    sorted.sort_unstable();
    assert_eq!(words, sorted, "KEYWORDS must be sorted by word");

    let unique: BTreeSet<&str> = words.iter().copied().collect();
    assert_eq!(unique.len(), words.len(), "KEYWORDS contains duplicates");
}

/// The compiler-known built-in type names the type parser dispatches on by
/// identifier text (`if name == "…"` in `parser/types.rs`). Read from source so
/// the registry cannot drift from what the parser actually recognises.
fn builtin_type_names_in_parser() -> BTreeSet<String> {
    let types_rs =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../bynk-syntax/src/parser/types.rs");
    let text = fs::read_to_string(&types_rs).unwrap();
    // `\b` anchors the boundary so this matches the bare `name == "…"` dispatch
    // idiom only — not a suffix like `type_name == "…"` (no word boundary before
    // `name` when preceded by `_`) nor an occurrence embedded in a longer ident.
    let re = regex::Regex::new(r#"\bname == "([A-Za-z][A-Za-z0-9_]*)""#).unwrap();
    re.captures_iter(&text).map(|c| c[1].to_string()).collect()
}

/// Drift guard (Gap B of #637): the built-in-type-name registry must equal the
/// set of names the type parser dispatches on. The registry drives both the
/// keyword reference page and the `bynk.resolve.reserved_builtin_type`
/// redeclaration diagnostic; if the parser gains or drops a built-in type the
/// registry (and this guard) must follow. Unlike the reserved keywords, these
/// names have no lexer token, so no other guard can see them.
#[test]
fn builtin_type_names_match_parser_dispatch() {
    let registered: BTreeSet<String> = BUILTIN_TYPE_NAMES
        .iter()
        .map(|k| k.word.to_string())
        .collect();
    let dispatched = builtin_type_names_in_parser();

    let missing: Vec<&String> = dispatched.difference(&registered).collect();
    let extra: Vec<&String> = registered.difference(&dispatched).collect();

    assert!(
        missing.is_empty(),
        "built-in type names dispatched in parser/types.rs but missing from \
         BUILTIN_TYPE_NAMES: {missing:#?}"
    );
    assert!(
        extra.is_empty(),
        "names in BUILTIN_TYPE_NAMES no longer dispatched in parser/types.rs: {extra:#?}"
    );

    let words: Vec<&str> = BUILTIN_TYPE_NAMES.iter().map(|k| k.word).collect();
    let mut sorted = words.clone();
    sorted.sort_unstable();
    assert_eq!(words, sorted, "BUILTIN_TYPE_NAMES must be sorted by name");
}

#[test]
fn registry_matches_lexer_tokens() {
    let lexed = keywords_in_lexer();
    let registered = registry_keywords();

    let missing: Vec<&String> = lexed.difference(&registered).collect();
    let extra: Vec<&String> = registered.difference(&lexed).collect();

    assert!(
        missing.is_empty(),
        "keywords in lexer.rs but missing from bynk_syntax::keywords::KEYWORDS: {missing:#?}"
    );
    assert!(
        extra.is_empty(),
        "keywords in KEYWORDS no longer declared in lexer.rs: {extra:#?}"
    );
}

#[test]
fn generated_keywords_page_is_up_to_date() {
    let page = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../site/src/content/docs/book/reference/keywords.md");
    let rendered = common::to_site_page(&render_markdown(), "reference");

    if std::env::var_os("BYNK_BLESS").is_some() {
        fs::write(&page, &rendered).unwrap();
        return;
    }

    let current = fs::read_to_string(&page).unwrap_or_default();
    assert_eq!(
        current, rendered,
        "site/src/content/docs/book/reference/keywords.md is out of date.\n\
         Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it keywords_reference::"
    );
}

/// #2464: the LLM primer lists every keyword by hand, so agents learn the
/// reserved words without a second fetch. It drifted once (59 words, several
/// retired), so its `## Keywords` section must name exactly the registry's
/// words, each in the right tier: one paragraph per tier, in the order the
/// keyword reference uses (hard, reserved-contextual, contextual, built-in type
/// names). A word in the wrong paragraph would teach an agent the wrong answer
/// to "can I use this as an identifier?".
#[test]
fn llms_primer_keywords_match_the_registry() {
    let primer = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/public/llms.txt");
    let text = fs::read_to_string(&primer).unwrap();
    let start = text
        .find("\n## Keywords\n")
        .expect("llms.txt has a `## Keywords` section");
    let body = &text[start + "\n## Keywords\n".len()..];
    let section = &body[..body.find("\n## ").unwrap_or(body.len())];

    let re = regex::Regex::new(r"`([A-Za-z][A-Za-z_]*)`").unwrap();
    let paragraphs: Vec<BTreeSet<String>> = section
        .split("\n\n")
        .map(|p| re.captures_iter(p).map(|c| c[1].to_string()).collect())
        .filter(|words: &BTreeSet<String>| !words.is_empty())
        .collect();

    let words = |it: &mut dyn Iterator<Item = &str>| -> BTreeSet<String> {
        it.map(str::to_string).collect()
    };
    let tiers = [
        (
            "hard keywords",
            words(
                &mut KEYWORDS
                    .iter()
                    .map(|k| k.word)
                    .filter(|w| !RESERVED_CONTEXTUAL.contains(w)),
            ),
        ),
        (
            "reserved-contextual keywords",
            words(&mut RESERVED_CONTEXTUAL.iter().copied()),
        ),
        (
            "contextual keywords",
            words(&mut CONTEXTUAL_KEYWORDS.iter().map(|k| k.word)),
        ),
        (
            "built-in type names",
            words(&mut BUILTIN_TYPE_NAMES.iter().map(|k| k.word)),
        ),
    ];

    assert_eq!(
        paragraphs.len(),
        tiers.len(),
        "site/public/llms.txt `## Keywords` must have one paragraph per tier \
         (hard, reserved-contextual, contextual, built-in type names)"
    );
    for ((tier, expected), listed) in tiers.iter().zip(&paragraphs) {
        let missing: Vec<&String> = expected.difference(listed).collect();
        let extra: Vec<&String> = listed.difference(expected).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "site/public/llms.txt `## Keywords`, {tier} paragraph, is out of step with \
             bynk_syntax::keywords:\n  missing: {missing:?}\n  not in this tier: {extra:?}"
        );
    }

    let hard = tiers[0].1.len();
    assert!(
        section.contains(&format!("**{hard}** hard keywords")),
        "site/public/llms.txt `## Keywords` must state `**{hard}** hard keywords`"
    );
}
