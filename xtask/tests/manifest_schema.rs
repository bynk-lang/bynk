//! Guard: #2096 — `bynk.toml`'s JSON Schema can't drift from the manifest the
//! toolchain reads.
//!
//! The schema is generated (`cargo xtask manifest-schema`) from the tables' own
//! types: `[fmt]` from `bynk-fmt`'s reader, the rest from
//! `bynk_project::manifest_schema`. These tests fail when the committed copy is
//! stale, when a table or key disagrees with `MANIFEST_TABLES` (the list
//! `check_manifest_str` refuses unknown names against), when a stated default
//! isn't the one the formatter uses, when the reference page's example manifest
//! names something the schema lacks, or when the VS Code extension stops
//! associating the schema with `bynk.toml`. `[lsp]` is held to the server's own
//! reader by `bynk-lsp`'s `lsp_section_matches_the_manifest_schema`.
//!
//! `BYNK_BLESS=1 cargo test -p xtask --test manifest_schema` rewrites the
//! committed schema.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use xtask::manifest_schema::{SCHEMA_PATH, SCHEMA_URL, generate, render};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &str) -> String {
    let path = repo().join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn table<'a>(schema: &'a Value, name: &str) -> &'a Value {
    &schema["properties"][name]
}

fn keys(object: &Value) -> BTreeSet<String> {
    object
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn the_committed_schema_is_current() {
    let fresh = render();
    if std::env::var_os("BYNK_BLESS").is_some() {
        xtask::manifest_schema::write(&repo()).expect("write the schema");
        return;
    }
    assert!(
        read(SCHEMA_PATH) == fresh,
        "{SCHEMA_PATH} is stale: run `cargo xtask manifest-schema` and commit the result"
    );
}

#[test]
fn the_schema_has_exactly_the_manifest_tables_and_keys() {
    let schema = generate();
    let tables: BTreeSet<String> = bynk_project::MANIFEST_TABLES
        .iter()
        .map(|(t, _)| (*t).to_string())
        .collect();
    assert_eq!(keys(&schema["properties"]), tables, "the schema's tables");
    for (name, expected) in bynk_project::MANIFEST_TABLES {
        // `[fmt]`'s keys are `bynk-fmt`'s (its list here is empty); the schema
        // is derived from that crate's reader, checked below.
        if *name == "fmt" {
            continue;
        }
        let expected: BTreeSet<String> = expected.iter().map(|k| (*k).to_string()).collect();
        assert_eq!(
            keys(&table(&schema, name)["properties"]),
            expected,
            "the keys of `[{name}]`"
        );
    }
}

#[test]
fn unknown_tables_and_keys_are_flagged() {
    // The CLIs refuse an unknown table or key (#1665); the schema says the same,
    // so the editor flags it before a build does.
    let schema = generate();
    assert_eq!(schema["additionalProperties"], false, "the root");
    for (name, _) in bynk_project::MANIFEST_TABLES {
        let t = table(&schema, name);
        assert_eq!(t["type"], "object", "`[{name}]` is a table");
        assert_eq!(t["additionalProperties"], false, "`[{name}]`");
        assert!(
            t["description"].as_str().is_some_and(|d| !d.is_empty()),
            "`[{name}]` has a description for hover"
        );
        for (key, value) in t["properties"].as_object().expect("properties") {
            assert!(
                value["description"].as_str().is_some_and(|d| !d.is_empty()),
                "`[{name}] {key}` has a description for hover"
            );
        }
    }
}

#[test]
fn the_fmt_table_states_the_formatters_defaults() {
    let schema = generate();
    let fmt = &table(&schema, "fmt")["properties"];
    let defaults = bynk_fmt::FormatOptions::default();
    let indent = match defaults.indent {
        bynk_fmt::IndentStyle::Tab => "tab",
        bynk_fmt::IndentStyle::Spaces(_) => "spaces",
    };
    assert_eq!(fmt["indent"]["default"], indent);
    assert_eq!(fmt["max_line_width"]["default"], defaults.max_line_width);
    assert_eq!(fmt["trailing_comma"]["default"], defaults.trailing_comma);
}

#[test]
fn the_fmt_reader_accepts_every_value_the_schema_offers() {
    let schema = generate();
    let fmt = &table(&schema, "fmt")["properties"];
    for (key, value) in fmt.as_object().expect("properties") {
        let candidates: Vec<Value> = match value["enum"].as_array() {
            Some(options) => options.clone(),
            None => vec![value["default"].clone()],
        };
        for candidate in candidates {
            let manifest = format!("[fmt]\n{key} = {candidate}\n");
            assert!(
                bynk_fmt::FmtConfig::from_manifest_str(&manifest).is_ok(),
                "the formatter refuses `{key} = {candidate}`, which the schema offers"
            );
        }
    }
    // And refuses what the schema flags.
    assert!(bynk_fmt::FmtConfig::from_manifest_str("[fmt]\nindent = \"tabs\"\n").is_err());
    assert!(bynk_fmt::FmtConfig::from_manifest_str("[fmt]\nmax_line_width = 0\n").is_err());
    assert!(bynk_fmt::FmtConfig::from_manifest_str("[fmt]\nmax_line_length = 9\n").is_err());
}

#[test]
fn the_reference_pages_example_manifest_matches_the_schema() {
    // `manifest.md` opens with a manifest that sets every key; each one must be
    // in the schema, and every key in the schema must be in it.
    let page = read("site/src/content/docs/docs/manifest.md");
    let example = page
        .split("```toml\n")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("manifest.md has a TOML example");
    let doc: toml::Table = example.parse().expect("the example parses");
    let schema = generate();
    let mut in_example = BTreeSet::new();
    for (name, value) in &doc {
        for key in value.as_table().expect("a table").keys() {
            in_example.insert(format!("{name}.{key}"));
        }
    }
    let mut in_schema = BTreeSet::new();
    for (name, t) in schema["properties"].as_object().expect("tables") {
        for key in keys(&t["properties"]) {
            in_schema.insert(format!("{name}.{key}"));
        }
    }
    assert_eq!(in_example, in_schema, "manifest.md's example vs the schema");
}

#[test]
fn the_extension_associates_the_schema_with_bynk_toml() {
    let package: Value =
        serde_json::from_str(&read("vscode-bynk/package.json")).expect("package.json parses");
    let rules = package["contributes"]["tomlValidation"]
        .as_array()
        .expect("vscode-bynk contributes `tomlValidation`");
    assert!(
        rules
            .iter()
            .any(|r| r["fileMatch"] == "bynk.toml" && r["url"] == SCHEMA_URL),
        "a `tomlValidation` rule maps `bynk.toml` to {SCHEMA_URL}"
    );
    assert_eq!(generate()["$id"], SCHEMA_URL);
}
