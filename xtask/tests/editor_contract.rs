//! Guard: the editor contract's committed snapshot can't drift from the
//! compiler. A change to the reserved keywords or the semantic-token legend
//! fails here until `cargo xtask editor-contract` is run and the diff
//! committed, which is how a compiler PR learns the VS Code extension needs a
//! matching change (the extension checks itself against the released contract).
//!
//! `BYNK_BLESS=1 cargo test -p xtask --test editor_contract` rewrites it.

use std::path::PathBuf;

use bynk_syntax::semantic_tokens::{STANDARD_MODIFIERS, STANDARD_TYPES};
use serde_json::Value;
use xtask::editor_contract::{SNAPSHOT_PATH, generate, render, render_release};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn the_committed_snapshot_is_current() {
    if std::env::var_os("BYNK_BLESS").is_some() {
        xtask::editor_contract::write(&repo()).expect("write the snapshot");
        return;
    }
    let committed = std::fs::read_to_string(repo().join(SNAPSHOT_PATH))
        .unwrap_or_else(|e| panic!("read {SNAPSHOT_PATH}: {e}"));
    assert!(
        committed == render(),
        "{SNAPSHOT_PATH} is stale: run `cargo xtask editor-contract` and commit the result. \
         The VS Code extension must be updated to match (see \
         design/vscode-extension-extraction.md)."
    );
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn custom_entries_are_exactly_the_legend_minus_the_standard_ones() {
    let contract = generate();
    let tokens = &contract["semanticTokens"];
    for (all, custom, standard) in [
        ("types", "customTypes", STANDARD_TYPES),
        ("modifiers", "customModifiers", STANDARD_MODIFIERS),
    ] {
        let expected: Vec<String> = strings(&tokens[all])
            .into_iter()
            .filter(|n| !standard.contains(&n.as_str()))
            .collect();
        assert!(!expected.is_empty(), "the legend has Bynk-custom {all}");
        assert_eq!(strings(&tokens[custom]), expected, "{custom}");
    }
}

#[test]
fn the_release_asset_is_the_snapshot_plus_a_version() {
    let release: Value = serde_json::from_str(&render_release("v1.2.3")).unwrap();
    assert_eq!(release["version"], "v1.2.3");
    let mut without = release.as_object().unwrap().clone();
    without.remove("version");
    assert_eq!(Value::Object(without), generate());
}

#[test]
fn every_reserved_keyword_is_listed() {
    let contract = generate();
    let words = contract["reservedKeywords"].as_array().unwrap();
    assert_eq!(words.len(), bynk_syntax::keywords::KEYWORDS.len());
    assert!(!words.is_empty());
}
