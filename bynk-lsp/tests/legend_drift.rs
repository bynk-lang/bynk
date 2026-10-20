//! v0.29 (ADR 0058): the legend drift guard.
//!
//! The semantic-token legend is the server's (`semantic_tokens_legend()`):
//! its array order is the wire encoding. The VS Code extension must declare
//! the SAME custom token-type / modifier *names* in `vscode-bynk/package.json`
//! (`contributes.semanticTokenTypes` / `semanticTokenModifiers`) so those
//! Bynk-distinctive tokens get themed — otherwise they silently render with
//! no colour. The names live in two languages (Rust + JSON); this is the one
//! enforceable cross-component contract: a single source of truth (the Rust
//! legend), checked against `package.json` here, so a legend change without
//! the matching extension edit fails CI.
//!
//! This file is `exclude`d from the published `bynk-lsp` crate (Cargo.toml) —
//! it reads `../vscode-bynk/package.json`, which is not in the crate tarball,
//! so a standalone `cargo test` on the published crate must not see it.

use bynk_lsp::index_queries;
use std::path::Path;

// bynk-lsp is a binary crate: include the pure module directly. `position`
// satisfies index_queries' one `crate::position` reference (the semantic-
// tokens producer converts spans for the delta encoding).

// The LSP-standard token types / modifiers the extension does NOT redeclare —
// VS Code provides and themes them. Everything else in the legend is
// Bynk-custom and MUST be declared (and scope-mapped) by the extension.
const STANDARD_TYPES: &[&str] = &[
    "type",
    "function",
    "variable",
    "method",
    "property",
    "decorator",
    // #1888: `--|` doc lines (no longer emitted since #2203; the legend is
    // append-only).
    "comment",
];
// #1888: `documentation` marked a `--|` doc line's `comment` token (no longer
// emitted since #2203; the legend is append-only).
const STANDARD_MODIFIERS: &[&str] = &["declaration", "documentation"];

fn package_json() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vscode-bynk/package.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("vscode-bynk/package.json is valid JSON")
}

/// The `id` of every entry in `contributes.<key>` (an array of `{id, …}`).
fn declared_ids(pkg: &serde_json::Value, key: &str) -> Vec<String> {
    pkg["contributes"][key]
        .as_array()
        .unwrap_or_else(|| panic!("contributes.{key} is an array"))
        .iter()
        .map(|e| {
            e["id"]
                .as_str()
                .unwrap_or_else(|| panic!("contributes.{key}[].id is a string"))
                .to_string()
        })
        .collect()
}

/// The legend's custom (non-standard) entries, in legend order.
fn custom(names: impl Iterator<Item = String>, standard: &[&str]) -> Vec<String> {
    names.filter(|n| !standard.contains(&n.as_str())).collect()
}

/// Compare as sets (sorted) — `package.json` declaration order is not
/// semantically meaningful; membership is. A mismatch in either direction
/// (legend gained a custom entry the extension lacks, or vice versa) fails.
fn assert_same_set(label: &str, mut legend: Vec<String>, mut declared: Vec<String>) {
    legend.sort();
    declared.sort();
    assert_eq!(
        legend, declared,
        "{label}: vscode-bynk/package.json must mirror the server legend's custom entries — \
         legend={legend:?}, package.json={declared:?}"
    );
}

#[test]
fn extension_mirrors_the_custom_token_types() {
    let legend = index_queries::semantic_tokens_legend();
    let custom_types = custom(
        legend.token_types.iter().map(|t| t.as_str().to_string()),
        STANDARD_TYPES,
    );
    let declared = declared_ids(&package_json(), "semanticTokenTypes");
    assert_same_set("semanticTokenTypes", custom_types, declared);
}

#[test]
fn extension_mirrors_the_custom_token_modifiers() {
    let legend = index_queries::semantic_tokens_legend();
    let custom_mods = custom(
        legend
            .token_modifiers
            .iter()
            .map(|m| m.as_str().to_string()),
        STANDARD_MODIFIERS,
    );
    let declared = declared_ids(&package_json(), "semanticTokenModifiers");
    assert_same_set("semanticTokenModifiers", custom_mods, declared);
}
