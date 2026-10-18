//! #2096: the JSON Schema for `bynk.toml`, generated from the manifest's Rust
//! types.
//!
//! The tables come from the crates that own them: `[fmt]` from `bynk-fmt`'s
//! `RawFmt` (the struct that reads it), the rest from
//! `bynk_project::manifest_schema`. The table list is
//! [`bynk_project::MANIFEST_TABLES`], the one `check_manifest_str` refuses
//! against, so a table added there without a schema fails [`generate`], and the
//! test holds every table's keys to it.
//!
//! The schema is committed at [`SCHEMA_PATH`], under the docs site's `public/`
//! directory, so the site serves it at [`SCHEMA_URL`]. `cargo xtask
//! manifest-schema` rewrites it; `tests/manifest_schema.rs` fails when it is
//! stale.

use std::path::Path;

use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

/// Where the schema is committed, relative to the repo root.
pub const SCHEMA_PATH: &str = "site/public/schemas/bynk.toml.json";

/// Where the docs site serves it: the schema's `$id`, the URL the VS Code
/// extension associates with `bynk.toml`, and the one a `#:schema` directive
/// names.
pub const SCHEMA_URL: &str = "https://bynk-lang.org/schemas/bynk.toml.json";

/// The manifest reference page, linked from each table's hover.
const DOCS_URL: &str = "https://bynk-lang.org/docs/manifest/";

/// The whole `bynk.toml` schema, keys sorted so the committed text is stable.
pub fn generate() -> Value {
    // Draft 07 is the dialect TOML language servers (taplo) support best, and
    // inlining keeps each table's description beside its key: taplo ignores
    // everything next to a `$ref`.
    let mut generator = SchemaSettings::draft07()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut properties = Map::new();
    for (table, _) in bynk_project::MANIFEST_TABLES {
        let schema = match *table {
            "fmt" => bynk_fmt::fmt_table_schema(&mut generator),
            other => bynk_project::manifest_schema::table_schema(other, &mut generator)
                .unwrap_or_else(|| panic!("`[{other}]` is in MANIFEST_TABLES but has no schema")),
        };
        let mut schema = without_null(schema.to_value());
        if let Value::Object(fields) = &mut schema {
            // The Rust type's name means nothing to someone editing a manifest.
            fields.remove("title");
            fields.insert(
                "x-taplo".into(),
                json!({ "links": { "key": format!("{DOCS_URL}#{table}") } }),
            );
        }
        properties.insert((*table).to_string(), schema);
    }
    let root = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": SCHEMA_URL,
        "title": "bynk.toml",
        "description": "The Bynk project manifest. Every table and key is optional. \
            Generated from the toolchain's manifest types by `cargo xtask manifest-schema`.",
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "x-taplo": { "links": { "key": DOCS_URL } },
    });
    sorted(root)
}

/// The schema as committed: pretty-printed, with a trailing newline.
pub fn render() -> String {
    let mut text = serde_json::to_string_pretty(&generate()).expect("a JSON value serialises");
    text.push('\n');
    text
}

/// Write the schema to [`SCHEMA_PATH`] under `root`.
pub fn write(root: &Path) -> std::io::Result<()> {
    let path = root.join(SCHEMA_PATH);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, render())
}

/// `schema` with the `null` that schemars allows for an `Option` field taken
/// out. TOML has no null: an optional key is simply absent, and `key = null`
/// doesn't parse, so offering it would only mislead. A `type` list loses its
/// `"null"`, and an `anyOf` left with one branch is folded into its parent
/// (the parent's own description wins over the branch's).
fn without_null(schema: Value) -> Value {
    let Value::Object(mut fields) = schema else {
        return match schema {
            Value::Array(items) => Value::Array(items.into_iter().map(without_null).collect()),
            other => other,
        };
    };
    if let Some(Value::Array(types)) = fields.get_mut("type") {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let only = types.remove(0);
            fields.insert("type".into(), only);
        }
    }
    if let Some(Value::Array(branches)) = fields.get_mut("anyOf") {
        branches.retain(|b| b != &json!({ "type": "null" }));
        // Fold only a lone object branch; anything else stays as it is, rather
        // than being taken out of the array and dropped.
        if let [Value::Object(branch)] = branches.as_slice() {
            let branch = branch.clone();
            fields.remove("anyOf");
            for (key, value) in branch {
                fields.entry(key).or_insert(value);
            }
        }
    }
    Value::Object(
        fields
            .into_iter()
            .map(|(k, v)| (k, without_null(v)))
            .collect(),
    )
}

/// `value` with every object's keys in sorted order, whichever map type
/// `serde_json` was built with (`preserve_order` keeps insertion order).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_null_drops_null_from_a_type_list() {
        assert_eq!(
            without_null(json!({ "type": ["string", "null"] })),
            json!({ "type": "string" })
        );
        assert_eq!(
            without_null(json!({ "type": ["string", "integer", "null"] })),
            json!({ "type": ["string", "integer"] })
        );
    }

    #[test]
    fn without_null_folds_a_lone_object_branch_keeping_the_parents_fields() {
        let folded = without_null(json!({
            "description": "outer",
            "anyOf": [{ "type": "string", "description": "inner" }, { "type": "null" }],
        }));
        assert_eq!(folded, json!({ "description": "outer", "type": "string" }));
    }

    #[test]
    fn without_null_keeps_an_any_of_it_cannot_fold() {
        // Two branches left: kept as a choice.
        let two = json!({ "anyOf": [{ "type": "string" }, { "type": "integer" }] });
        assert_eq!(without_null(two.clone()), two);
        // One non-object branch (`true` is a valid schema): kept, not dropped.
        assert_eq!(
            without_null(json!({ "anyOf": [true, { "type": "null" }] })),
            json!({ "anyOf": [true] })
        );
    }
}
