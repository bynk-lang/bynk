//! The grammar docs-delta: the annotated reference must cover every production.
//!
//! This is the enforcing companion to the A2b authoring effort. It guarantees:
//!   1. **Bijection** — every embeddable grammar rule (everything except the
//!      trivial wrappers the display layer collapses) has *exactly one*
//!      `{{#grammar <rule>}}` entry in the Book's `reference/grammar.md`.
//!   2. **Valid args** — every `{{#grammar <x>}}` / `{{#grammar-semantics <x>}}`
//!      across the Book names a real top-level rule (catching typos the remark
//!      directive plugin would only flag at build, and only for `{{#grammar}}`).
//!   3. **Anchors** — every entry carries a `{#rule-<raw>}` heading id matching
//!      its `{{#grammar <raw>}}`, so the diagnostics `Construct` deep-links
//!      resolve; one per embeddable rule, unique.
//!   4. **The spec covers the grammar** (#1780) — every embeddable rule is
//!      embedded at least once under `book/spec/`, so a new production can't
//!      land in the reference alone and leave the normative grammar behind.
//!
//! Backslash-escaped directives (`\{{#…}}`, as used to show the syntax literally
//! in the contributor guide) are ignored.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn grammar_json() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tree-sitter-bynk/src/grammar.json");
    fs::read_to_string(path).expect("read grammar.json")
}

fn docs_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/content/docs/book")
}

fn grammar_md() -> String {
    fs::read_to_string(docs_src().join("reference/grammar.md")).expect("read grammar.md")
}

fn gather_md(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            gather_md(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Every directive use across the docs as `(kind, rule)` pairs, where `kind` is
/// `"grammar"` or `"grammar-semantics"`. Backslash-escaped uses are skipped.
fn directive_uses(text: &str) -> Vec<(String, String)> {
    let re = regex::Regex::new(r"(\\?)\{\{#(grammar(?:-semantics)?) ([A-Za-z_]+)\}\}").unwrap();
    re.captures_iter(text)
        .filter(|c| &c[1] != "\\")
        .map(|c| (c[2].to_string(), c[3].to_string()))
        .collect()
}

/// The set of all top-level rule names in the grammar.
fn all_rule_names() -> BTreeSet<String> {
    let grammar: Value = serde_json::from_str(&grammar_json()).unwrap();
    grammar
        .get("rules")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn every_embeddable_rule_has_exactly_one_entry() {
    let embeddable: BTreeSet<String> = xtask::grammar::embeddable_rules(&grammar_json())
        .into_iter()
        .collect();

    // Count `{{#grammar <raw>}}` uses (not `-semantics`) in grammar.md.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (kind, rule) in directive_uses(&grammar_md()) {
        if kind == "grammar" {
            *counts.entry(rule).or_default() += 1;
        }
    }

    let embedded: BTreeSet<String> = counts.keys().cloned().collect();

    let missing: Vec<&String> = embeddable.difference(&embedded).collect();
    assert!(
        missing.is_empty(),
        "grammar rules with no `{{#grammar …}}` entry in grammar.md: {missing:#?}\n\
         Add an entry (heading + `{{#grammar <rule>}}` + `{{#rule-<rule>}}` anchor) for each."
    );

    let extra: Vec<&String> = embedded.difference(&embeddable).collect();
    assert!(
        extra.is_empty(),
        "grammar.md embeds rules that are not embeddable (collapsed wrappers or unknown): {extra:#?}"
    );

    let duped: Vec<(&String, usize)> = counts
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(r, n)| (r, *n))
        .collect();
    assert!(
        duped.is_empty(),
        "rules embedded more than once in grammar.md: {duped:#?}"
    );
}

#[test]
fn every_directive_arg_is_a_real_rule() {
    let rules = all_rule_names();
    let mut files = Vec::new();
    gather_md(&docs_src(), &mut files);
    files.sort();

    let mut bad: Vec<String> = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let rel = file.strip_prefix(docs_src()).unwrap_or(&file).display();
        for (kind, rule) in directive_uses(&text) {
            if !rules.contains(&rule) {
                bad.push(format!(
                    "{rel}: {{{{#{kind} {rule}}}}} — `{rule}` is not a grammar rule"
                ));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "directive(s) naming an unknown grammar rule:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn every_entry_has_a_matching_anchor() {
    let embeddable: BTreeSet<String> = xtask::grammar::embeddable_rules(&grammar_json())
        .into_iter()
        .collect();
    let md = grammar_md();

    // Heading ids of the form `{#rule-<raw>}`.
    let anchor_re = regex::Regex::new(r"\{#rule-([A-Za-z_]+)\}").unwrap();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in anchor_re.captures_iter(&md) {
        *counts.entry(c[1].to_string()).or_default() += 1;
    }
    let anchored: BTreeSet<String> = counts.keys().cloned().collect();

    let missing: Vec<&String> = embeddable.difference(&anchored).collect();
    assert!(
        missing.is_empty(),
        "entries missing a `{{#rule-<raw>}}` heading anchor: {missing:#?}\n\
         Append `{{#rule-<rule>}}` to each entry heading so deep-links resolve."
    );

    let extra: Vec<&String> = anchored.difference(&embeddable).collect();
    assert!(
        extra.is_empty(),
        "`{{#rule-…}}` anchors that do not match an embeddable rule: {extra:#?}"
    );

    let duped: Vec<(&String, usize)> = counts
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(r, n)| (r, *n))
        .collect();
    assert!(
        duped.is_empty(),
        "duplicate `{{#rule-…}}` anchors: {duped:#?}"
    );

    // Each entry pairs a `{{#grammar <raw>}}` with the same `{#rule-<raw>}`:
    // both sets equal `embeddable`, already asserted, so the pairing holds.
}

/// #1780: the normative spec embeds every embeddable rule somewhere under
/// `book/spec/` (the syntactic grammar, or §3 for the lexical ones), and nothing
/// that isn't one. Before this check only the reference was held to the
/// grammar, and 29 productions reached it without reaching the spec (8 added by
/// #1669, the other 21 by #1780).
#[test]
fn every_embeddable_rule_is_in_the_spec() {
    let embeddable: BTreeSet<String> = xtask::grammar::embeddable_rules(&grammar_json())
        .into_iter()
        .collect();
    let mut files = Vec::new();
    gather_md(&docs_src().join("spec"), &mut files);
    let embedded: BTreeSet<String> = files
        .iter()
        .flat_map(|f| directive_uses(&fs::read_to_string(f).unwrap()))
        .filter(|(kind, _)| kind == "grammar")
        .map(|(_, rule)| rule)
        .collect();
    let missing: Vec<&String> = embeddable.difference(&embedded).collect();
    assert!(
        missing.is_empty(),
        "grammar rules the spec never embeds: {missing:#?}\n\
         Add a `{{{{#grammar <rule>}}}}` entry for each to the spec's grammar chapter \
         (`book/spec/syntactic-grammar.md`, or `lexical-grammar.md` for a token)."
    );
    let extra: Vec<&String> = embedded.difference(&embeddable).collect();
    assert!(
        extra.is_empty(),
        "the spec embeds rules that are not embeddable (collapsed wrappers): {extra:#?}"
    );
}
