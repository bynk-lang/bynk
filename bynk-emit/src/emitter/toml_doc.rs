//! P7.3 (#1303): a minimal typed TOML tree and printer — the one piece of
//! R7.8 (`Artefacts` is a keyed set of typed documents, never a `String` at
//! construction) landable ahead of `bynk-ts` (Arc B, P7.5+), since
//! `wrangler.toml` is the one document `bynk-emit` produces that isn't
//! TypeScript. Not a general TOML library: `TomlValue` represents exactly
//! what `emitter::wrangler::emit_wrangler_toml` needs to build a
//! `wrangler.toml` today, no more.
//!
//! [`print_toml_document`] is the *only* function in this crate that writes
//! TOML syntax — `emit_wrangler_toml` builds a [`TomlDocument`], this module
//! renders it. That split is what makes string-escaping a printer guarantee
//! (every `TomlValue::Str` is escaped unconditionally, §"Decision B" of
//! #1303) rather than a per-call-site judgement call, which is what left
//! `wrangler.rs`'s own `name`/`binding`/`class_name` values unescaped before
//! this — safe today only because those particular values happen to be
//! compiler-derived identifiers that can't contain a TOML-breaking
//! character, not because anything enforced it structurally.

use std::fmt::Write as _;

/// A TOML document: the root block's entries (review of #1304, finding 3 —
/// a field set once at construction, not a `TomlBlock` a caller could
/// accidentally `push_block` out of first position, which `push_block`
/// itself couldn't have rejected: TOML has no marker distinguishing "the
/// root block" from "a table block with no header" once both are just
/// entries in the same list) followed by any number of headed blocks. Every
/// block, printed — root included — is followed by exactly one blank line
/// (including the last — confirmed against every current
/// `expected/**/wrangler.toml` golden fixture, which all end in a trailing
/// blank line).
pub struct TomlDocument {
    header_comment: &'static str,
    root: Vec<TomlEntry>,
    blocks: Vec<TomlBlock>,
}

impl TomlDocument {
    /// `header_comment` is the leading `# …` line's text *without* the `#`
    /// marker — [`print_toml_document`] prepends it, the same convention
    /// [`TomlEntry::with_comment`] already used (review of #1304, finding
    /// 2: the two used to disagree, one taking a marker-inclusive literal
    /// and the other marker-exclusive, with nothing enforcing either).
    pub(crate) fn new(header_comment: &'static str, root: Vec<TomlEntry>) -> Self {
        Self {
            header_comment,
            root,
            blocks: Vec::new(),
        }
    }

    pub(crate) fn push_block(&mut self, block: TomlBlock) {
        self.blocks.push(block);
    }

    /// Set the root `main` entry's value in place — the structural,
    /// tree-level equivalent of the old text-based `toml_edit` patch this
    /// replaces (P7.6, #1309, Decision E): a caller that already holds the
    /// real tree (`bynk-strip::strip_project_to_js`) uses this instead of
    /// printing then re-parsing just to change one field.
    ///
    /// Returns `false`, changing nothing, if the document has no root
    /// `main` entry — the caller's job to treat that as an error (P7.4,
    /// #1305's own guardrail against a silently-unpatched JS artefact whose
    /// manifest still names the stripped `.ts` entry: this method reports
    /// the miss, it doesn't decide it's fine).
    #[must_use]
    pub fn set_main(&mut self, value: impl Into<String>) -> bool {
        for entry in &mut self.root {
            if entry.key == "main" {
                entry.value = TomlValue::Str(value.into());
                return true;
            }
        }
        false
    }
}

/// One `[path]` or `[[path]]` section plus its `key = value` entries, in
/// order. Always headed — the document's own root block is
/// [`TomlDocument`]'s own field, not constructible as a `TomlBlock` (review
/// of #1304, finding 3).
pub(crate) struct TomlBlock {
    header: TomlHeader,
    entries: Vec<TomlEntry>,
}

enum TomlHeader {
    Table(&'static str),
    ArrayTable(&'static str),
    /// #1796: `[parent.key]`, where `key` is chosen at emit time rather than
    /// fixed by the generator (a Durable Object class name under `exports`).
    /// The printer renders `key` bare when it is a TOML bare key and as an
    /// escaped basic string otherwise, so a key, like a value, can't break
    /// out of its header whatever it contains.
    KeyedTable(&'static str, String),
}

impl TomlBlock {
    pub(crate) fn table(path: &'static str, entries: Vec<TomlEntry>) -> Self {
        Self {
            header: TomlHeader::Table(path),
            entries,
        }
    }

    pub(crate) fn array_table(path: &'static str, entries: Vec<TomlEntry>) -> Self {
        Self {
            header: TomlHeader::ArrayTable(path),
            entries,
        }
    }

    /// `[parent.key]` — one entry of a table keyed by a runtime name, such as
    /// `[exports.Alpha]` (#1796).
    pub(crate) fn keyed_table(
        parent: &'static str,
        key: impl Into<String>,
        entries: Vec<TomlEntry>,
    ) -> Self {
        Self {
            header: TomlHeader::KeyedTable(parent, key.into()),
            entries,
        }
    }
}

/// One `key = value` line, with an optional trailing `# comment` — TOML's
/// own comment syntax, not an escape hatch; `wrangler.toml`'s one instance
/// today is the KV namespace id's `# set at deploy time`.
pub(crate) struct TomlEntry {
    key: &'static str,
    value: TomlValue,
    comment: Option<&'static str>,
}

impl TomlEntry {
    pub(crate) fn kv(key: &'static str, value: TomlValue) -> Self {
        Self {
            key,
            value,
            comment: None,
        }
    }

    pub(crate) fn with_comment(key: &'static str, value: TomlValue, comment: &'static str) -> Self {
        Self {
            key,
            value,
            comment: Some(comment),
        }
    }
}

/// Exactly the value shapes `wrangler.toml` generation writes today — a
/// basic string (always escaped on render, unconditionally), a bare
/// integer, a bool, an array (rendered as `[a, b, …]`, TOML's inline-array
/// form), and an inline table (`{ k = v, … }`, standard-library P3's
/// `simple = { limit = 100, period = 60 }`). No float and no nesting beyond
/// one section level: none of those appear in the current output, and
/// widening this when a real future value needs it is cheap.
pub(crate) enum TomlValue {
    Str(String),
    Int(i64),
    /// #1883, slice 1b: `[assets] run_worker_first = false`.
    Bool(bool),
    Array(Vec<TomlValue>),
    /// P3 (#2342): an inline table, its keys bare, in the order given.
    InlineTable(Vec<(&'static str, TomlValue)>),
}

impl TomlValue {
    pub(crate) fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }
}

/// Render `doc` to TOML text. The one function in this module — and, per
/// this file's own module doc, in `bynk-emit`'s TOML-producing surface —
/// that calls `write!`/`writeln!`/`format!` to build TOML syntax.
pub fn print_toml_document(doc: &TomlDocument) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", doc.header_comment);
    print_entries(&mut out, &doc.root);
    let _ = writeln!(out);
    for block in &doc.blocks {
        match &block.header {
            TomlHeader::Table(path) => {
                let _ = writeln!(out, "[{path}]");
            }
            TomlHeader::ArrayTable(path) => {
                let _ = writeln!(out, "[[{path}]]");
            }
            TomlHeader::KeyedTable(parent, key) => {
                let _ = writeln!(out, "[{parent}.{}]", render_key(key));
            }
        }
        print_entries(&mut out, &block.entries);
        let _ = writeln!(out);
    }
    out
}

fn print_entries(out: &mut String, entries: &[TomlEntry]) {
    for entry in entries {
        let value = render_value(&entry.value);
        match entry.comment {
            Some(comment) => {
                let _ = writeln!(out, "{} = {value} # {comment}", entry.key);
            }
            None => {
                let _ = writeln!(out, "{} = {value}", entry.key);
            }
        }
    }
}

/// A key segment, bare when TOML allows it (`A-Za-z0-9_-`, non-empty) and a
/// quoted basic string otherwise — the same escaping [`render_value`] gives
/// every string value.
fn render_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        key.to_string()
    } else {
        format!("\"{}\"", escape_toml_basic_string(key))
    }
}

fn render_value(value: &TomlValue) -> String {
    match value {
        TomlValue::Str(s) => format!("\"{}\"", escape_toml_basic_string(s)),
        TomlValue::Int(n) => n.to_string(),
        TomlValue::Bool(b) => b.to_string(),
        TomlValue::Array(items) => {
            let rendered: Vec<String> = items.iter().map(render_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        TomlValue::InlineTable(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{} = {}", render_key(k), render_value(v)))
                .collect();
            format!("{{ {} }}", rendered.join(", "))
        }
    }
}

/// Escape a source string literal for interpolation into a TOML *basic*
/// string (the `"…"` form). Queue names and cron expressions come from user
/// string literals, which can decode to contain `"`, `\`, newline and tab
/// (`bynk-syntax/src/lexer.rs`) — all of which would otherwise break out of
/// the TOML string and inject config keys. Every character we escape maps
/// to a valid TOML compact escape; remaining control characters fall back
/// to the `\uXXXX` form so the output is always a well-formed basic string.
///
/// Applied unconditionally by [`render_value`] to every [`TomlValue::Str`] —
/// not just the values a caller happens to know are user-supplied. Relocated
/// here from `emitter/wrangler.rs` (P7.3, #1303): escaping is the printer's
/// job now, applied structurally to every string this module renders, not a
/// per-call-site judgement about which particular value might need it.
fn escape_toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            // Control characters have no compact TOML escape besides the ones
            // above and must not appear raw in a basic string.
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_toml_basic_string_neutralises_injection() {
        // The trigger from the defect report: a queue name whose decoded value
        // carries a quote + newline would otherwise close the string and inject
        // a config key.
        assert_eq!(
            escape_toml_basic_string("q\nkey = \"injected"),
            "q\\nkey = \\\"injected"
        );
        assert_eq!(escape_toml_basic_string("a\\b"), "a\\\\b");
        assert_eq!(escape_toml_basic_string("a\tb"), "a\\tb");
    }

    #[test]
    fn set_main_changes_only_the_main_entry() {
        let mut doc = TomlDocument::new(
            "Generated by bynkc — do not edit by hand.",
            vec![
                TomlEntry::kv("name", TomlValue::str("api")),
                TomlEntry::kv("main", TomlValue::str("index.ts")),
            ],
        );
        doc.push_block(TomlBlock::table(
            "triggers",
            vec![TomlEntry::kv(
                "crons",
                TomlValue::Array(vec![TomlValue::str("*/5 * * * *")]),
            )],
        ));

        assert!(doc.set_main("index.js"));

        let text = print_toml_document(&doc);
        let parsed: toml::Table = text.parse().expect("valid TOML");
        assert_eq!(parsed["main"].as_str(), Some("index.js"));
        assert_eq!(parsed["name"].as_str(), Some("api"));
        let crons = parsed["triggers"]["crons"].as_array().expect("crons");
        assert_eq!(crons[0].as_str(), Some("*/5 * * * *"));
    }

    #[test]
    fn set_main_reports_a_missing_root_main_entry_rather_than_swallowing_it() {
        let mut doc = TomlDocument::new(
            "Generated by bynkc — do not edit by hand.",
            vec![TomlEntry::kv("name", TomlValue::str("api"))],
        );
        assert!(!doc.set_main("index.js"));
        let parsed: toml::Table = print_toml_document(&doc).parse().expect("valid TOML");
        assert!(parsed.get("main").is_none());
    }

    #[test]
    fn escape_toml_basic_string_passes_plain_values_through() {
        // Ordinary cron expressions and queue names are untouched.
        assert_eq!(escape_toml_basic_string("*/5 * * * *"), "*/5 * * * *");
        assert_eq!(escape_toml_basic_string("order-events"), "order-events");
    }

    #[test]
    fn escape_toml_basic_string_escapes_other_control_chars() {
        // A NUL has no compact escape and must not appear raw in a basic string.
        assert_eq!(escape_toml_basic_string("a\u{0}b"), "a\\u0000b");
        assert_eq!(escape_toml_basic_string("a\u{7f}b"), "a\\u007Fb");
    }

    #[test]
    fn escaped_value_is_valid_toml_and_round_trips() {
        // The security invariant, enforced by a real TOML parser (not a golden
        // byte-compare): interpolating the escaped value produces a well-formed
        // single-key table whose decoded value is *exactly* the input — no
        // injected keys, no broken string. Covers the injection payload from the
        // defect report plus a control char that takes the `\uXXXX` fallback.
        for input in ["q\nkey = \"injected", "*/5 * * * *\\\"", "a\u{0}b\ttail"] {
            let doc = format!("queue = \"{}\"", escape_toml_basic_string(input));
            let table: toml::Table = doc
                .parse()
                .unwrap_or_else(|e| panic!("escaped {input:?} is invalid TOML: {e} ({doc:?})"));
            assert_eq!(
                table.len(),
                1,
                "escaped {input:?} injected extra keys: {table:?}"
            );
            assert_eq!(
                table["queue"].as_str(),
                Some(input),
                "escaped {input:?} did not round-trip"
            );
        }
    }

    #[test]
    fn print_toml_document_renders_a_representative_document_and_round_trips() {
        // Not a golden byte-compare (that's `bless_positive_fixtures`'s job) —
        // this proves the printer's *general* shape (root block, a `[[…]]`
        // array table, a `[…]` table, an array value, a commented entry) is
        // well-formed TOML a real parser accepts, with every value surviving
        // exactly. `TomlBlock`/`TomlEntry` construction mirrors
        // `emit_wrangler_toml`'s own shape one-for-one.
        //
        // Review of #1304, finding 1: the `name` and the one `crons` element
        // below are the injection payload from the defect report, not an
        // ordinary identifier — every value the *golden* corpus carries today
        // is compiler-derived and never needs escaping, which means a
        // zero-diff `bless_positive_fixtures` run cannot tell an escaping
        // `render_value` from a `render_value` that stopped escaping
        // entirely. This is the one test standing between "the printer
        // escapes every string" (this module's whole point) and that claim
        // quietly going false — it has to drive a hostile value *through*
        // `print_toml_document`, not just through `escape_toml_basic_string`
        // directly (the tests above) or a hand-built fragment (the test
        // below).
        let hostile = "q\nkey = \"injected";
        let mut doc = TomlDocument::new(
            "Generated by bynkc — do not edit by hand.",
            vec![
                TomlEntry::kv("name", TomlValue::str(hostile)),
                TomlEntry::kv("main", TomlValue::str("index.ts")),
            ],
        );
        doc.push_block(TomlBlock::array_table(
            "services",
            vec![
                TomlEntry::kv("binding", TomlValue::str("COMMERCE_PAYMENT")),
                TomlEntry::kv("service", TomlValue::str("commerce-payment")),
            ],
        ));
        doc.push_block(TomlBlock::array_table(
            "kv_namespaces",
            vec![
                TomlEntry::kv("binding", TomlValue::str("BYNK_KV")),
                TomlEntry::with_comment(
                    "id",
                    TomlValue::str("<KV_NAMESPACE_ID>"),
                    "set at deploy time",
                ),
            ],
        ));
        doc.push_block(TomlBlock::table(
            "triggers",
            vec![TomlEntry::kv(
                "crons",
                TomlValue::Array(vec![TomlValue::str("*/5 * * * *"), TomlValue::str(hostile)]),
            )],
        ));

        let text = print_toml_document(&doc);
        let parsed: toml::Table = text
            .parse()
            .unwrap_or_else(|e| panic!("printer produced invalid TOML: {e}\n{text}"));

        assert_eq!(
            parsed.len(),
            5,
            "the hostile `name` value injected extra root keys: {parsed:?}"
        );
        assert_eq!(parsed["name"].as_str(), Some(hostile));
        assert_eq!(parsed["main"].as_str(), Some("index.ts"));
        let services = parsed["services"].as_array().expect("services array");
        assert_eq!(services.len(), 1);
        assert_eq!(services[0]["binding"].as_str(), Some("COMMERCE_PAYMENT"));
        let kv = parsed["kv_namespaces"].as_array().expect("kv array");
        assert_eq!(kv[0]["id"].as_str(), Some("<KV_NAMESPACE_ID>"));
        let crons = parsed["triggers"]["crons"].as_array().expect("crons array");
        assert_eq!(crons.len(), 2, "the hostile crons element broke the array");
        assert_eq!(crons[0].as_str(), Some("*/5 * * * *"));
        assert_eq!(crons[1].as_str(), Some(hostile));

        // Every block, including the last, is followed by exactly one blank
        // line — the shape every current golden `wrangler.toml` fixture has.
        assert!(text.ends_with("\n\n"));
    }

    #[test]
    fn keyed_table_keys_are_bare_when_they_can_be_and_quoted_otherwise() {
        // #1796: `[exports.<Class>]`. A Bynk class name is always a bare key
        // (`__EventsFanout` included), but the printer, not the caller, owns
        // that guarantee, so a hostile key must stay inside its header too.
        let hostile = "a.b]\nx = \"y";
        let mut doc = TomlDocument::new("t", vec![]);
        for key in ["Alpha", "__EventsFanout", hostile] {
            doc.push_block(TomlBlock::keyed_table(
                "exports",
                key,
                vec![TomlEntry::kv("type", TomlValue::str("durable-object"))],
            ));
        }
        let text = print_toml_document(&doc);
        assert!(text.contains("[exports.Alpha]\n"), "{text}");
        assert!(text.contains("[exports.__EventsFanout]\n"), "{text}");

        let parsed: toml::Table = text
            .parse()
            .unwrap_or_else(|e| panic!("printer produced invalid TOML: {e}\n{text}"));
        assert_eq!(parsed.len(), 1, "a key escaped its header: {parsed:?}");
        let exports = parsed["exports"].as_table().expect("exports table");
        assert_eq!(exports.len(), 3);
        assert_eq!(exports[hostile]["type"].as_str(), Some("durable-object"));
    }
}
