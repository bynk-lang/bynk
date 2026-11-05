//! Render the `tree-sitter-bynk` grammar to EBNF.
//!
//! This crate is the single source of the grammar reference. It takes the
//! compiled grammar JSON (`tree-sitter-bynk/src/grammar.json`) as input and is
//! otherwise location-agnostic, so the same renderer feeds both the full
//! appendix page ([`render_appendix`]) and the per-rule includes embedded in
//! the curated reference page ([`render_production`] / [`render_rule`]). Because
//! both come from one implementation, an embedded production cannot drift from
//! the appendix.
//!
//! **Display names.** Grammar rule names are parser-internal (`_type_ref`,
//! `_expression`, …). For the reference we render *readable* names via
//! [`display_name`]: a trivial `_x ::= y` wrapper collapses to its target, an
//! optional override applies, otherwise a single leading underscore is stripped.
//! The transform is applied to both rule heads and the nonterminal references
//! inside productions, so the whole reference reads as language, not internals.
//!
//! See `bynkc/tests/grammar_reference.rs` (the appendix generator) and
//! `site/src/plugins/remark-bynk-directives.mjs` (the `{{#grammar <rule>}}`
//! include directive the Book renders with).

use std::error::Error;
use std::fmt;

use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

/// An error rendering a grammar production.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrammarError {
    /// The grammar JSON could not be parsed.
    Parse(String),
    /// `name` is not a top-level rule in the grammar.
    UnknownRule(String),
}

impl fmt::Display for GrammarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrammarError::Parse(e) => write!(f, "could not parse grammar JSON: {e}"),
            GrammarError::UnknownRule(name) => {
                write!(
                    f,
                    "unknown grammar rule `{name}` (not a top-level production)"
                )
            }
        }
    }
}

impl Error for GrammarError {}

/// Display-name overrides for rules whose mechanical name reads badly. Each key
/// must be a real top-level rule (checked in tests). Keep this tiny — most rules
/// read fine after collapsing wrappers and stripping the leading underscore.
const OVERRIDES: &[(&str, &str)] = &[];

/// A parsed grammar document plus the source order of its top-level rules.
///
/// `serde_json`'s object map is key-sorted unless the workspace-unifying
/// `preserve_order` feature is on, so the rule order the reference depends on
/// is read here, from the text, and never from the map.
struct Grammar {
    doc: Value,
    order: Vec<String>,
}

impl Grammar {
    fn parse(grammar_json: &str) -> Result<Grammar, serde_json::Error> {
        let doc: Value = serde_json::from_str(grammar_json)?;
        let order = serde_json::from_str::<RuleOrder>(grammar_json)?.0;
        Ok(Grammar { doc, order })
    }

    fn rules(&self) -> Option<Rules<'_>> {
        let map = self.doc.get("rules").and_then(Value::as_object)?;
        Some(Rules {
            map,
            order: &self.order,
        })
    }
}

/// The top-level rules of a [`Grammar`], iterated in source order.
#[derive(Clone, Copy)]
struct Rules<'a> {
    map: &'a Map<String, Value>,
    order: &'a [String],
}

impl<'a> Rules<'a> {
    fn get(&self, name: &str) -> Option<&'a Value> {
        self.map.get(name)
    }

    fn contains_key(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    fn keys(&self) -> impl Iterator<Item = &'a String> + use<'a> {
        self.order.iter()
    }

    fn iter(&self) -> impl Iterator<Item = (&'a String, &'a Value)> + use<'a> {
        let map = self.map;
        self.order
            .iter()
            .filter_map(move |k| map.get(k).map(|v| (k, v)))
    }
}

/// Deserialises only the key order of the top-level `rules` object.
struct RuleOrder(Vec<String>);

impl<'de> Deserialize<'de> for RuleOrder {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Doc(Vec<String>);
        struct DocVisitor;
        impl<'de> Visitor<'de> for DocVisitor {
            type Value = Doc;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a grammar object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Doc, A::Error> {
                let mut order = Vec::new();
                while let Some(key) = m.next_key::<String>()? {
                    if key == "rules" {
                        order = m.next_value::<KeysOnly>()?.0;
                    } else {
                        m.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(Doc(order))
            }
        }
        impl<'de> Deserialize<'de> for Doc {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                d.deserialize_map(DocVisitor)
            }
        }
        Ok(RuleOrder(Doc::deserialize(d)?.0))
    }
}

/// An object's keys, in text order, with the values skipped.
struct KeysOnly(Vec<String>);

impl<'de> Deserialize<'de> for KeysOnly {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = KeysOnly;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<KeysOnly, A::Error> {
                let mut keys = Vec::new();
                while let Some(k) = m.next_key::<String>()? {
                    m.next_value::<IgnoredAny>()?;
                    // A repeated key keeps its last value in the map, so list
                    // it once, at its first position.
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
                Ok(KeysOnly(keys))
            }
        }
        d.deserialize_map(V)
    }
}

/// If `name`'s body is a single `SYMBOL` (a trivial `_x ::= y` wrapper), return
/// the target rule name. Such wrappers are collapsed: they never appear as their
/// own production, and references to them render as the target's display name.
fn trivial_wrapper_target<'a>(rules: Rules<'a>, name: &str) -> Option<&'a str> {
    let body = rules.get(name)?;
    if body.get("type").and_then(Value::as_str) == Some("SYMBOL") {
        body.get("name").and_then(Value::as_str)
    } else {
        None
    }
}

/// The readable display name of a grammar rule: collapse a trivial-wrapper chain
/// to its target, apply any override, else strip a single leading underscore.
fn display_name_in(rules: Rules<'_>, name: &str) -> String {
    if let Some(target) = trivial_wrapper_target(rules, name) {
        return display_name_in(rules, target);
    }
    if let Some((_, disp)) = OVERRIDES.iter().find(|(k, _)| *k == name) {
        return (*disp).to_string();
    }
    name.strip_prefix('_').unwrap_or(name).to_string()
}

/// Render a grammar node to EBNF text plus a precedence level:
/// 0 = choice (`a | b`), 1 = sequence (`a b`), 2 = atom / postfix (`x`, `(…)*`).
/// Nonterminal (`SYMBOL`) references are rendered with their display name.
fn render(rules: Rules<'_>, node: &Value) -> (String, u8) {
    match node.get("type").and_then(Value::as_str).unwrap_or("") {
        "SYMBOL" => (
            display_name_in(rules, node["name"].as_str().unwrap_or("?")),
            2,
        ),
        "STRING" => (format!("\"{}\"", node["value"].as_str().unwrap_or("")), 2),
        "PATTERN" => (format!("/{}/", node["value"].as_str().unwrap_or("")), 2),
        "BLANK" => ("ε".to_string(), 2),
        // Wrappers that don't affect the surface grammar: render their content.
        "PREC" | "PREC_LEFT" | "PREC_RIGHT" | "PREC_DYNAMIC" | "TOKEN" | "IMMEDIATE_TOKEN"
        | "FIELD" | "ALIAS" => render(rules, &node["content"]),
        "REPEAT" => (format!("{}*", wrap_atom(rules, &node["content"])), 2),
        "REPEAT1" => (format!("{}+", wrap_atom(rules, &node["content"])), 2),
        "SEQ" => {
            let parts: Vec<String> = members(node).iter().map(|m| wrap(rules, m, 1)).collect();
            (parts.join(" "), 1)
        }
        "CHOICE" => {
            let all = members(node);
            let has_blank = all
                .iter()
                .any(|m| m.get("type").and_then(Value::as_str) == Some("BLANK"));
            let non_blank: Vec<&Value> = all
                .iter()
                .filter(|m| m.get("type").and_then(Value::as_str) != Some("BLANK"))
                .collect();
            if has_blank {
                // An optional: `X?`.
                if non_blank.len() == 1 {
                    (format!("{}?", wrap_atom(rules, non_blank[0])), 2)
                } else {
                    let inner: Vec<String> = non_blank.iter().map(|m| render(rules, m).0).collect();
                    (format!("({})?", inner.join(" | ")), 2)
                }
            } else {
                let inner: Vec<String> = non_blank.iter().map(|m| render(rules, m).0).collect();
                (inner.join(" | "), 0)
            }
        }
        other => (format!("/* {other} */"), 2),
    }
}

fn members(node: &Value) -> Vec<Value> {
    node["members"].as_array().cloned().unwrap_or_default()
}

/// Wrap so the result can be a postfix operand (`*`, `+`, `?`): needs an atom.
fn wrap_atom(rules: Rules<'_>, node: &Value) -> String {
    wrap(rules, node, 2)
}

/// Wrap `node`'s rendering in parens if its level is below `min`.
fn wrap(rules: Rules<'_>, node: &Value, min: u8) -> String {
    let (s, level) = render(rules, node);
    if level < min { format!("({s})") } else { s }
}

fn render_extra(node: &Value) -> String {
    match node.get("type").and_then(Value::as_str).unwrap_or("") {
        "SYMBOL" => format!("`{}`", node["name"].as_str().unwrap_or("?")),
        "PATTERN" => format!("`/{}/`", node["value"].as_str().unwrap_or("")),
        "STRING" => format!("`\"{}\"`", node["value"].as_str().unwrap_or("")),
        _ => "?".to_string(),
    }
}

/// Render the complete grammar reference appendix
/// (`site/src/content/docs/book/reference/grammar-appendix.md`): the
/// generated-file header, the
/// notation note, the full `ebnf` block of every production (display names,
/// trivial wrappers collapsed), and the Tokens & trivia section.
pub fn render_appendix(grammar_json: &str) -> String {
    let grammar = Grammar::parse(grammar_json).expect("grammar.json parses");

    let mut out = String::new();
    out.push_str("# Complete grammar (appendix)\n\n");
    out.push_str(
        "<!-- GENERATED FILE — do not edit by hand.\n     \
         Source: tree-sitter-bynk/src/grammar.json, via bynkc/tests/grammar_reference.rs.\n     \
         Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it grammar_reference:: -->\n\n",
    );
    out.push_str(
        "The complete Bynk grammar, generated from the `tree-sitter-bynk` grammar. \
         For the annotated, per-construct reference see [Syntax & grammar](grammar.md).\n\n",
    );
    out.push_str("**Notation.** ");
    out.push_str(
        "`\"x\"` a literal token · `/x/` a regular expression · `( … )?` optional · \
         `( … )*` zero or more · `( … )+` one or more · `a | b` choice · `ε` empty. \
         Rule names are the readable display names (a leading `_` denotes an \
         internal helper rule; trivial wrappers are collapsed). `doc_block` is an \
         external token — a `--- … ---` documentation block.\n\n",
    );

    out.push_str("```ebnf\n");
    if let Some(rules) = grammar.rules() {
        for (name, body) in rules.iter() {
            // Trivial wrappers are collapsed into their target.
            if trivial_wrapper_target(rules, name).is_some() {
                continue;
            }
            let (rendered, _) = render(rules, body);
            out.push_str(&format!(
                "{} ::= {rendered}\n",
                display_name_in(rules, name)
            ));
        }
    }
    out.push_str("```\n\n");

    out.push_str("## Tokens & trivia\n\n");
    if let Some(word) = grammar.doc.get("word").and_then(Value::as_str) {
        out.push_str(&format!("- **Word token:** `{word}`\n"));
    }
    if let Some(extras) = grammar.doc.get("extras").and_then(Value::as_array) {
        let rendered: Vec<String> = extras.iter().map(render_extra).collect();
        out.push_str(&format!(
            "- **Ignored between tokens:** {}\n",
            rendered.join(", ")
        ));
    }
    if let Some(externals) = grammar.doc.get("externals").and_then(Value::as_array) {
        let rendered: Vec<String> = externals.iter().map(render_extra).collect();
        out.push_str(&format!("- **External tokens:** {}\n", rendered.join(", ")));
    }

    out
}

/// Look up a rule body by name, erroring if the grammar is unparseable or the
/// rule is not a top-level production.
fn rule_body<'a>(rules: Rules<'a>, name: &str) -> Result<&'a Value, GrammarError> {
    rules
        .get(name)
        .ok_or_else(|| GrammarError::UnknownRule(name.to_string()))
}

/// Render a single production's right-hand side (display names applied), exactly
/// as it appears after `<name> ::= ` in the appendix's EBNF block.
///
/// Errors if the grammar JSON cannot be parsed, or if `name` is not a top-level
/// rule of the grammar.
pub fn render_rule(grammar_json: &str, name: &str) -> Result<String, GrammarError> {
    let grammar = Grammar::parse(grammar_json).map_err(|e| GrammarError::Parse(e.to_string()))?;
    let rules = grammar
        .rules()
        .ok_or_else(|| GrammarError::UnknownRule(name.to_string()))?;
    let body = rule_body(rules, name)?;
    Ok(render(rules, body).0)
}

/// Render a complete production line, `<display name> ::= <rhs>`, as it appears
/// in the appendix (no surrounding fence). This is what `{{#grammar <rule>}}`
/// embeds.
pub fn render_production(grammar_json: &str, name: &str) -> Result<String, GrammarError> {
    let grammar = Grammar::parse(grammar_json).map_err(|e| GrammarError::Parse(e.to_string()))?;
    let rules = grammar
        .rules()
        .ok_or_else(|| GrammarError::UnknownRule(name.to_string()))?;
    let body = rule_body(rules, name)?;
    Ok(format!(
        "{} ::= {}",
        display_name_in(rules, name),
        render(rules, body).0
    ))
}

/// Every top-level rule that should have exactly one `{{#grammar}}` entry in the
/// annotated reference: all rules **except** the trivial wrappers the display
/// layer collapses (so this can never disagree with what is rendered). Grammar
/// rule order is preserved. Returns an empty vector if the JSON is unparseable.
pub fn embeddable_rules(grammar_json: &str) -> Vec<String> {
    let Ok(grammar) = Grammar::parse(grammar_json) else {
        return Vec::new();
    };
    let Some(rules) = grammar.rules() else {
        return Vec::new();
    };
    rules
        .keys()
        .filter(|name| trivial_wrapper_target(rules, name.as_str()).is_none())
        .cloned()
        .collect()
}

/// The readable display name for a top-level rule. Errors if the grammar is
/// unparseable or `name` is not a top-level rule.
pub fn display_name(grammar_json: &str, name: &str) -> Result<String, GrammarError> {
    let grammar = Grammar::parse(grammar_json).map_err(|e| GrammarError::Parse(e.to_string()))?;
    let rules = grammar
        .rules()
        .ok_or_else(|| GrammarError::UnknownRule(name.to_string()))?;
    if !rules.contains_key(name) {
        return Err(GrammarError::UnknownRule(name.to_string()));
    }
    Ok(display_name_in(rules, name))
}

/// Render the grammar as the JSON document the documentation site consumes. The
/// `{{#grammar}}` remark directive looks each rule up in `productions` (the same
/// `<name> ::= <rhs>` line [`render_production`] embeds, over [`embeddable_rules`]),
/// and the full-grammar page reads `appendix` ([`render_appendix`]). Because both
/// come from the same renderer as the mdBook preprocessor, the site cannot drift
/// from the book. Object keys preserve grammar order (read from the source text, not from
/// `serde_json`'s map).
///
/// The committed artifact (`site/src/generated/grammar.json`) is drift-guarded by
/// `bynk-grammar/tests/generated_grammar_json.rs`; regenerate with `BYNK_BLESS=1`.
pub fn render_site_json(grammar_json: &str) -> String {
    /// A JSON object written in the order given, whatever `serde_json`'s map is.
    struct Ordered(Vec<(String, String)>);
    impl Serialize for Ordered {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.collect_map(self.0.iter().map(|(k, v)| (k, v)))
        }
    }
    /// Fields serialise in declaration order.
    #[derive(Serialize)]
    struct SiteDoc {
        _generated: &'static str,
        productions: Ordered,
        appendix: String,
    }

    let mut productions = Vec::new();
    for rule in embeddable_rules(grammar_json) {
        if let Ok(production) = render_production(grammar_json, &rule) {
            productions.push((rule, production));
        }
    }
    let doc = SiteDoc {
        _generated: "GENERATED from tree-sitter-bynk/src/grammar.json. Do not edit by hand. \
             Regenerate with: BYNK_BLESS=1 cargo test -p bynk-grammar --test generated_grammar_json",
        productions: Ordered(productions),
        appendix: render_appendix(grammar_json),
    };
    let mut out = serde_json::to_string_pretty(&doc).expect("serialise grammar JSON");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    fn grammar_json() -> String {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tree-sitter-bynk/src/grammar.json");
        fs::read_to_string(path).expect("read grammar.json")
    }

    #[test]
    fn render_rule_uses_display_names() {
        let g = grammar_json();
        // `_pattern`/`_expression` render without their leading underscore.
        // #472: a match arm's pattern is `pattern | refined_pattern` (the
        // latter admitted only here, not through `_pattern` generally).
        assert_eq!(
            render_rule(&g, "match_arm").unwrap(),
            "(pattern | refined_pattern) (\"if\" expression)? \"=>\" expression \",\"?"
        );
        // `http_method` is a plain choice, unchanged.
        assert_eq!(
            render_rule(&g, "http_method").unwrap(),
            "\"GET\" | \"POST\" | \"PUT\" | \"PATCH\" | \"DELETE\""
        );
        // `http_handler` references `_type_ref` and `block` — display, not raw.
        let http = render_rule(&g, "http_handler").unwrap();
        assert!(http.contains("type_ref"), "{http}");
        assert!(!http.contains("_type_ref"), "{http}");
        assert!(http.contains("block"), "{http}");
    }

    #[test]
    fn embeddable_rules_excludes_trivial_wrappers() {
        let g = grammar_json();
        let rules = embeddable_rules(&g);
        // v0.17 added: adapter_decl, _adapter_body_item, binding_decl,
        // binding_requirement. v0.20a added: function_type_ref, lambda_expr,
        // lambda_param. v0.20b added: list_literal. v0.21 added:
        // float_literal. v0.43 added: string_interpolation. v0.44 added:
        // service_protocol. v0.45 added: actor_decl, scheme, by_clause. v0.51
        // added: scheme_config, scheme_arg. v0.79 added: effect_send_stmt. v0.80
        // added: invariant_decl. v0.81 added: store_field, store_kind,
        // assign_stmt. v0.85 added: store_annotation, annotation_arg. v0.96
        // removed: state_decl, commit_stmt (parity cutover, ADR 0123).
        // v0.103 added: ws_open_handler, ws_close_handler (the `from websocket`
        // lifecycle handlers; `on message` reuses queue_handler). v0.114 added:
        // property_decl, for_all, for_all_binding (generative tests); mock_expr/
        // mock_arg renamed to val_expr/val_arg (no count change). v0.115 added:
        // requires_clause, ensures_clause (function contracts). v0.116 added:
        // transition_decl (agent step invariants). v0.117 added: observation_expr,
        // trace_expr (the observation surface; the call-count matcher is inlined).
        // v0.118 (testing track slice 6) removed: mocks_decl, integration_decl,
        // wires_decl, _integration_body_item (mocks/integration/wires retired) and
        // added: provides_clause (the test-scope stub; the `as <tier>` clause and
        // the stub right-hand side are inlined). Net -3. (#548 renamed
        // provides_clause → stub_clause with the `stub` keyword — no count change.)
        // v0.130 added:
        // literal_pattern (literal match-arm patterns). v0.131 (ADR 0159) added:
        // cors_policy, cors_field (the CORS service policy). Net +3. v0.141
        // (ADR 0164) added: security_policy, security_field (the security-headers
        // service policy). Net +2. v0.142 (ADR 0165) added: limits_policy,
        // limits_field (the request-body-size service policy). Net +2. v0.145
        // (ADR 0169) removed: positional_binding — a variant payload is now a
        // full `_pattern` (recursion), so a positional binding is a payload-less
        // `variant_pattern`, not a dedicated node. Net -1. v0.146 (ADR 0170)
        // added: do_stmt (the `do e` effect statement). Net +1. v0.157 (ADR
        // 0183) added: applied_type_ref (a user generic-type application
        // `Name[Arg, …]`). Net +1.
        // v0.184 (ADR 0205) added: call_site_actor (the test-body
        // `by <Actor>(<identity>)` clause). Net +1.
        // Slice C added: wire_expr (the `Wire(<String>)` raw system-tier
        // argument). Net +1.
        // #472 added: refined_pattern (`_ where <predicate>` match-arm
        // patterns, admitted only at a match arm's top-level pattern). Net +1.
        // #474 added: or_pattern (`p₁ | p₂`) and paren_pattern (transparent
        // grouping around a pattern, e.g. after `is`). Net +2.
        // message-bundles slice 1 (#859) added: messages_decl, message_entry
        // (the `messages <tag> { "code" => "template" }` construct). Net +2.
        // Events track slice 0/1 (spine #936) added: event_decl (`event Name =
        // { fields }`), event_handler (`on event(...)`), event_pattern,
        // event_pattern_field, event_pattern_value (the slice-1 subscription
        // filter `from Events(E { field: value, .. })`; `service_protocol`
        // itself was extended in place, no new rule for it). Net +5.
        // Events track slice 4 (spine #936) added: schema_dispatch_clause
        // (the `via schema(N)` envelope-version dispatch clause; nested
        // inside `service_protocol`'s Events arm, extended in place). Net +1.
        // #1706 added: faults_expr (the `expect <call> faults` fault claim).
        // Net +1.
        // #1888 added: doc_line (the `--|` one-line doc token, an extra).
        // Net +1.
        // #1883 (static assets, slice 1a) added: asset_decl. Net +1.
        // #2010 added: let_else_stmt, let_else_match (`let <pattern> … else …`),
        // and _let_pattern, _let_variant_pattern, _let_or_pattern (the patterns
        // that may open one, so a bare name after `let` stays a binding). Net +5.
        // #2137 added: advance_stmt (`advance Clock by <Duration>`). Net +1.
        // #2134 added: env_clause, env_entry (a suite's or case's
        // `env { NAME = "value" … }`). Net +2.
        // #2203 removed: doc_line (the one-line doc is an external token now,
        // scanned with the doc-block). Net -1.
        // #2224 added: migrate_decl, migrate_drop (an agent's `migrate from N`
        // step and its `drop` item). Net +2.
        // #2248 added: removed_agent_decl (`removed agent Old`). Net +1.
        // #2341 added: rejected_handler (`on rejected(why: …)`). Net +1.
        // #2347 follow-up added: _contextual_identifier (`trace`/`advance` as
        // ordinary identifiers outside their keyword shapes). Net +1.
        // #2387 (embedded text, slice 1) added: embed_decl. Net +1.
        // #2641 (record patterns) added: record_pattern, record_pattern_field,
        // and _is_pattern, _is_base_pattern, _is_or_pattern (the patterns an
        // `is` takes unparenthesised, which exclude a record pattern). Net +5.
        // #2617 (named constants) added: const_decl. Net +1.
        // #2640 (concurrent effects, slice 0) added: par_stmt (the `par { … }`
        // binding block) and _par_statement (what it may hold: every statement
        // but a `let … else`). Net +2.
        // Constrained generics slice 0 added: type_param, where_clause,
        // where_bound (`constraint` is a trivial wrapper over `identifier`,
        // so the display layer collapses it). Net +3.
        // #2654 (`let` destructuring) added: _let_paren_record_pattern (the
        // parenthesised record pattern of an `else`-less `let`). Net +1.
        // Named constants (#2617) added const_decl. Net +1.
        // #2618 (private declarations) added: visibility_modifier. Net +1.
        // #2680 (list patterns) added: list_pattern and list_rest. Net +2.
        // #2699 (range patterns) added: int_range. Net +1.
        // #2712 (lambda record-pattern parameters) added: _lambda_record_pattern and
        // _lambda_record_field. Net +2.
        // #2619 (raw and multi-line strings) added: escape_sequence (the
        // external `raw_string` token is not a rule). Net +1.
        // #2622 (`Decimal`) added: decimal_literal. Net +1.
        // #2841 (suite `fixture`) added: fixture_decl. Net +1.
        // #2842 (snapshot claims) added: snapshot_expr. Net +1.
        // #2858 (plain-JSON responses) added: json_policy, json_field (the
        // response-JSON service policy). Net +2.
        // #2890 (the `consumes_item` wrapper) added: consumes_item. Net +1.
        assert_eq!(rules.len(), 192);
        assert!(rules.iter().any(|r| r == "http_handler"));
        assert!(rules.iter().any(|r| r == "_type_ref"));
        // The two trivial wrappers the display layer collapses are excluded.
        assert!(!rules.iter().any(|r| r == "_base_type"));
        assert!(!rules.iter().any(|r| r == "pred_atom"));
        // Unparseable JSON yields no rules rather than panicking.
        assert!(embeddable_rules("not json").is_empty());
    }

    #[test]
    fn render_production_includes_display_head() {
        let g = grammar_json();
        assert_eq!(
            render_production(&g, "match_arm").unwrap(),
            "match_arm ::= (pattern | refined_pattern) (\"if\" expression)? \"=>\" expression \",\"?"
        );
    }

    #[test]
    fn display_name_collapses_and_strips() {
        let g = grammar_json();
        // Trivial wrapper `_base_type ::= base_type` collapses to its target.
        assert_eq!(display_name(&g, "_base_type").unwrap(), "base_type");
        // Helper rules strip the leading underscore.
        assert_eq!(display_name(&g, "_expression").unwrap(), "expression");
        assert_eq!(display_name(&g, "_type_ref").unwrap(), "type_ref");
        // An ordinary rule is unchanged.
        assert_eq!(display_name(&g, "http_handler").unwrap(), "http_handler");
    }

    #[test]
    fn render_rule_unknown_rule_errors() {
        let g = grammar_json();
        assert_eq!(
            render_rule(&g, "no_such_rule"),
            Err(GrammarError::UnknownRule("no_such_rule".to_string()))
        );
    }

    #[test]
    fn render_rule_invalid_json_errors() {
        assert!(matches!(
            render_rule("not json", "match_arm"),
            Err(GrammarError::Parse(_))
        ));
    }

    #[test]
    fn override_keys_are_real_rules() {
        let g = grammar_json();
        for (key, _) in OVERRIDES {
            assert!(
                display_name(&g, key).is_ok(),
                "override key `{key}` is not a top-level rule"
            );
        }
    }

    /// The display transform must not map two displayed productions to the same
    /// name — that would make the reference ambiguous. Trivial wrappers are
    /// collapsed and so excluded.
    #[test]
    fn display_names_are_unique() {
        let g = grammar_json();
        let grammar = Grammar::parse(&g).unwrap();
        let rules = grammar.rules().unwrap();
        let mut seen: HashMap<String, String> = HashMap::new();
        for name in rules.keys() {
            if trivial_wrapper_target(rules, name).is_some() {
                continue;
            }
            let disp = display_name_in(rules, name);
            if let Some(prev) = seen.insert(disp.clone(), name.clone()) {
                panic!("display name `{disp}` for `{name}` collides with `{prev}`");
            }
        }
    }

    /// Pins the two renderers to one implementation: every displayed rule's
    /// production line must appear verbatim in the appendix, and the appendix
    /// has exactly one production per non-wrapper rule (wrappers are collapsed,
    /// nothing is duplicated).
    #[test]
    fn every_displayed_rule_matches_the_appendix() {
        let g = grammar_json();
        let appendix = render_appendix(&g);
        let grammar = Grammar::parse(&g).unwrap();
        let rules = grammar.rules().unwrap();

        let mut displayed = 0;
        for name in rules.keys() {
            if trivial_wrapper_target(rules, name).is_some() {
                continue;
            }
            displayed += 1;
            let line = render_production(&g, name).unwrap();
            assert!(
                appendix.contains(&line),
                "production for `{name}` not found in appendix:\n{line}"
            );
        }

        // One `::=` per displayed rule — collapsed wrappers added no lines and
        // no production is duplicated. (No grammar token contains `::=`.)
        assert_eq!(appendix.matches("::=").count(), displayed);
    }

    /// Rule order is the source order of the text, not the (key-sorted) order
    /// of `serde_json`'s map, so it can't depend on which crates share a build.
    #[test]
    fn rule_order_follows_the_source_text() {
        let g = r#"{"rules":{"zeta":{"type":"BLANK"},"alpha":{"type":"BLANK"},"mid":{"type":"BLANK"}}}"#;
        assert_eq!(embeddable_rules(g), ["zeta", "alpha", "mid"]);
        let appendix = render_appendix(g);
        let z = appendix.find("zeta ::=").unwrap();
        let a = appendix.find("alpha ::=").unwrap();
        let m = appendix.find("mid ::=").unwrap();
        assert!(z < a && a < m);
        let site = render_site_json(g);
        let z = site.find("\"zeta\"").unwrap();
        let a = site.find("\"alpha\"").unwrap();
        let m = site.find("\"mid\"").unwrap();
        assert!(z < a && a < m);
    }
}
