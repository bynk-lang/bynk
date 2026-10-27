//! The editor contract: the compiler-owned facts a language-client extension
//! must stay in step with, as one JSON document.
//!
//! The VS Code extension keeps a TextMate grammar (it highlights by keyword
//! list) and declares the custom semantic-token types the server emits. Both
//! must follow the compiler, and both used to be checked by Rust tests that read
//! `../vscode-bynk` (`textmate_keywords`, `legend_drift`). That only works while
//! the extension lives in this repository. This document is the same data,
//! published: the extension checks itself against the contract of the server
//! release it pins (see `design/vscode-extension-extraction.md`, §4).
//!
//! Two forms:
//! - the **snapshot**, committed at [`SNAPSHOT_PATH`]. It omits the version, so
//!   a release bump never makes it stale; a change to the keywords or legend
//!   shows up as a diff in the compiler PR that makes it, which is the signal
//!   that the extension needs a matching change. `tests/editor_contract.rs`
//!   fails when it is stale;
//! - the **release asset** `bynk-editor-contract-<tag>.json`, the snapshot plus
//!   the release's version ([`render_release`]).

use std::path::Path;

use bynk_syntax::keywords::KEYWORDS;
use bynk_syntax::semantic_tokens::{
    STANDARD_MODIFIERS, STANDARD_TYPES, TOKEN_MODIFIERS, TOKEN_TYPES,
};
use serde_json::{Value, json};

/// Where the snapshot is committed, relative to the repo root.
pub const SNAPSHOT_PATH: &str = "bynk-lsp/editor-contract.json";

fn custom(names: &[String], standard: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|n| !standard.contains(&n.as_str()))
        .cloned()
        .collect()
}

/// The contract without a version.
pub fn generate() -> Value {
    let types: Vec<String> = TOKEN_TYPES.iter().map(|t| (*t).to_string()).collect();
    let modifiers: Vec<String> = TOKEN_MODIFIERS.iter().map(|m| (*m).to_string()).collect();
    json!({
        "reservedKeywords": KEYWORDS.iter().map(|k| k.word).collect::<Vec<_>>(),
        "semanticTokens": {
            // Full, in wire order (the legend is append-only).
            "types": types,
            "modifiers": modifiers,
            // The entries the extension must declare in package.json.
            "customTypes": custom(&types, STANDARD_TYPES),
            "customModifiers": custom(&modifiers, STANDARD_MODIFIERS),
        },
    })
}

/// `value` with every object's keys in sorted order, whichever map type
/// `serde_json` was built with: `bynk-grammar` turns on `preserve_order`, and
/// cargo unifies features across a workspace build, so the same `json!` would
/// otherwise render differently under `cargo test --workspace` and `-p xtask`.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn pretty(value: &Value) -> String {
    let mut text =
        serde_json::to_string_pretty(&sorted(value.clone())).expect("a JSON value serialises");
    text.push('\n');
    text
}

/// The snapshot as committed: pretty-printed, with a trailing newline.
pub fn render() -> String {
    pretty(&generate())
}

/// The release asset: the snapshot plus the release `tag`.
pub fn render_release(tag: &str) -> String {
    let Value::Object(mut fields) = generate() else {
        unreachable!("the contract is an object")
    };
    fields.insert("version".into(), json!(tag));
    pretty(&Value::Object(fields))
}

/// Write the snapshot to [`SNAPSHOT_PATH`] under `root`.
pub fn write(root: &Path) -> std::io::Result<()> {
    std::fs::write(root.join(SNAPSHOT_PATH), render())
}
