//! Guard: the editor contract's committed snapshot can't drift from the
//! compiler. A change to the reserved keywords or the semantic-token legend
//! fails here until `cargo xtask editor-contract` is run and the diff
//! committed, which is how a compiler PR learns the VS Code extension needs a
//! matching change (the extension checks itself against the released contract).
//!
//! `BYNK_BLESS=1 cargo test -p xtask --test editor_contract` rewrites it.

use std::path::PathBuf;

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
        (
            "types",
            "customTypes",
            &[
                "type",
                "function",
                "variable",
                "method",
                "property",
                "decorator",
                "comment",
            ][..],
        ),
        (
            "modifiers",
            "customModifiers",
            &["declaration", "documentation"][..],
        ),
    ] {
        let expected: Vec<String> = strings(&tokens[all])
            .into_iter()
            .filter(|n| !standard.contains(&n.as_str()))
            .collect();
        assert!(!expected.is_empty(), "the legend has Bynk-custom {all}");
        assert_eq!(strings(&tokens[custom]), expected, "{custom}");
    }
}

/// While the Rust drift guard (`bynk-lsp/tests/legend_drift.rs`) and this
/// contract each hold a list of the standard token types, the extension's
/// declarations must agree with the contract too, so the two lists cannot
/// drift apart unnoticed. Goes with `legend_drift` when the extension checks
/// itself against the released contract.
#[test]
fn the_extension_declares_exactly_the_contracts_custom_entries() {
    let text = std::fs::read_to_string(repo().join("vscode-bynk/package.json")).unwrap();
    let pkg: Value = serde_json::from_str(&text).unwrap();
    let contract = generate();
    for (key, custom) in [
        ("semanticTokenTypes", "customTypes"),
        ("semanticTokenModifiers", "customModifiers"),
    ] {
        let mut declared: Vec<String> = pkg["contributes"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect();
        let mut want = strings(&contract["semanticTokens"][custom]);
        declared.sort();
        want.sort();
        assert_eq!(declared, want, "{key}");
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
