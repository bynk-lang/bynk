//! Reserved keyword ↔ TextMate grammar drift guard (storage-track deliverable).
//!
//! The docs site highlights code blocks with a TextMate grammar
//! (`site/src/grammars/bynk.tmLanguage.json`), NOT the tree-sitter grammar. It is
//! a copy of the VS Code extension's (bynk-lang/vscode-bynk), which checks its
//! own against the released editor contract.
//! Twice now a keyword shipped unhighlighted because only the tree-sitter
//! `highlights.scm` was updated (`invariant`/`implies` in v0.80, fixed in #300).
//! This test makes the lag structurally impossible: every reserved keyword in the
//! registry must appear, as a whole word, in some TextMate match pattern.
//!
//! It does NOT assert the converse (the grammar may highlight non-reserved
//! contextual words like `store`/`key`, the HTTP methods, or refinement
//! predicates), only that no reserved keyword is left dark.

use std::fs;
use std::path::PathBuf;

use bynk_syntax::keywords::KEYWORDS;

fn tmlanguage() -> (PathBuf, String) {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/grammars/bynk.tmLanguage.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    (path, text)
}

/// Whether `word` occurs as a whole word (alphanumeric/`_` boundaries) anywhere
/// in the grammar — i.e. inside some `"match"` alternation like `(a|word|b)`.
fn whole_word_present(haystack: &str, word: &str) -> bool {
    let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
    haystack.match_indices(word).any(|(i, _)| {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + word.len()..].chars().next();
        boundary(before) && boundary(after)
    })
}

#[test]
fn every_reserved_keyword_is_highlighted_in_textmate() {
    let (path, text) = tmlanguage();
    let missing: Vec<&str> = KEYWORDS
        .iter()
        .map(|k| k.word)
        .filter(|w| !whole_word_present(&text, w))
        .collect();
    assert!(
        missing.is_empty(),
        "reserved keywords not highlighted in {}: {missing:?}\n\
         Add each to an appropriate `keywords`/`types` match pattern so the editor \
         does not silently lag the language.",
        path.display()
    );
}
