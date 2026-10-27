//! #2459: the opt-in hint for an unresolved doc link.
//!
//! A `[Name]` in a doc comment that resolves to nothing renders as plain text
//! (the render-only decision, #848), so a typo or a link broken by an edit goes
//! unnoticed. This module turns each unresolved candidate into a low-severity
//! diagnostic, `bynk.doc.unresolved_link`, with the closest in-scope names as
//! did-you-mean suggestions the code action offers as quick fixes.
//!
//! LSP-only: the checker and `bynk check` never see it. The severity is the
//! `bynk.docLinks.reportUnresolved` setting (`off` / `hint` / `warning`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use bynk_check::index::ProjectIndex;
use bynk_check::locals::LocalBinding;
use bynk_syntax::ast::SourceUnit;
use bynk_syntax::span::Span;
use serde_json::json;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, DiagnosticSeverity,
    DocumentChanges, NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier, Range,
    TextDocumentEdit, TextEdit, Url, WorkspaceEdit,
};

use crate::index_queries::{
    DOC_LINK_BARE_KINDS, DOC_LINK_MEMBER_KINDS, DocLinkContext, own_part_names, resolve_doc_link_in,
};

/// The diagnostic code. Registered in `bynk_syntax::diagnostics::REGISTRY`.
pub const CODE: &str = "bynk.doc.unresolved_link";

/// How many did-you-mean names a diagnostic carries.
const MAX_SUGGESTIONS: usize = 3;

/// The `bynk.docLinks.reportUnresolved` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    Off,
    #[default]
    Hint,
    Warning,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "off" => Some(Level::Off),
            "hint" => Some(Level::Hint),
            "warning" => Some(Level::Warning),
            _ => None,
        }
    }

    /// Read the setting out of a `workspace/didChangeConfiguration` payload or
    /// `initializationOptions`: `{ "bynk": { "docLinks": { "reportUnresolved": … } } }`
    /// or the section itself, `{ "docLinks": { … } }`.
    pub fn from_settings(settings: &serde_json::Value) -> Option<Level> {
        let section = settings.get("bynk").unwrap_or(settings);
        let value = section.get("docLinks")?.get("reportUnresolved")?.as_str()?;
        Level::parse(value)
    }

    fn severity(self) -> Option<DiagnosticSeverity> {
        match self {
            Level::Off => None,
            Level::Hint => Some(DiagnosticSeverity::HINT),
            Level::Warning => Some(DiagnosticSeverity::WARNING),
        }
    }
}

/// Top-level item names declared by the first-party (synthetic) units. Their
/// symbols are never in the project index, so a link to one (`[Clock.now]`)
/// reads as unresolved even though the name is real; those are exempt until
/// first-party linking lands (#2455), after which this is redundant but
/// harmless.
static FIRST_PARTY_NAMES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let mut names = HashSet::new();
    for (_, src) in bynk_check::firstparty::FIRSTPARTY_SOURCES {
        let Ok(tokens) = bynk_syntax::lexer::tokenize(src) else {
            continue;
        };
        let (Some(unit), _) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, src) else {
            continue;
        };
        let items = match &unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Suite(_) => continue,
        };
        names.extend(
            items
                .iter()
                .filter_map(|i| i.name())
                .map(|n| n.name.clone()),
        );
    }
    names
});

/// Does `name` (`Owner` or `Owner.member`) name a first-party item?
fn is_first_party(name: &str) -> bool {
    let owner = name.split_once('.').map_or(name, |(o, _)| o);
    FIRST_PARTY_NAMES.contains(owner)
}

/// Is the candidate at `span` an intentional bracket — escaped (`\[TODO]`) or
/// inside an inline code span (`` `[TODO]` ``)?
fn is_escaped(text: &str, span: Span) -> bool {
    if text[..span.start].ends_with('\\') {
        return true;
    }
    let line_start = text[..span.start].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..span.start].matches('`').count() % 2 == 1
}

/// Optimal-string-alignment distance (a transposition costs one edit), over
/// lowercased chars so a case slip is one edit at most.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut rows = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(rows[i - 2][j - 2] + 1);
            }
            rows[i][j] = d;
        }
    }
    rows[a.len()][b.len()]
}

/// The closest names to `name`, nearest first (ties alphabetical), within a
/// third of the name's length in edits: those in `owner_unit`'s resolution
/// scope, and (for a bare name) the attached declaration's `own_parts`.
fn suggestions(
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
    own_parts: &[String],
) -> Vec<String> {
    let scope = doc_scope.get(owner_unit).map_or(&[][..], Vec::as_slice);
    let kinds = if name.contains('.') {
        DOC_LINK_MEMBER_KINDS
    } else {
        DOC_LINK_BARE_KINDS
    };
    let budget = (name.chars().count() / 3).max(1);
    let mut found: Vec<(usize, &str)> = index
        .symbols
        .iter()
        .filter(|(k, e)| e.def.is_some() && kinds.contains(&k.kind) && scope.contains(&k.unit))
        .map(|(k, _)| k.name.as_str())
        .chain(
            own_parts
                .iter()
                .map(String::as_str)
                .filter(|_| !name.contains('.')),
        )
        .map(|n| (distance(name, n), n))
        .filter(|&(d, n)| d <= budget && n != name)
        .collect();
    found.sort();
    found.dedup_by_key(|&mut (_, n)| n);
    found
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, n)| n.to_string())
        .collect()
}

/// A diagnostic for every unresolved doc link in `text`, a unit's source whose
/// own name is `owner_unit` and whose project-relative path is `path`. Links
/// resolve exactly as document links do (#2454): the attached declaration's own
/// parts first (`locals` supplies its parameters), then the unit scope. Empty
/// when `level` is `Off`.
pub fn unresolved_link_diagnostics(
    text: &str,
    path: &Path,
    owner_unit: &str,
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    locals: &[LocalBinding],
    level: Level,
) -> Vec<Diagnostic> {
    let Some(severity) = level.severity() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for link in crate::symbols::doc_link_spans_attached(text) {
        let (name, span) = (link.name, link.span);
        let ctx = link.attached.map(|attached| DocLinkContext {
            path,
            text,
            attached,
            locals,
        });
        if resolve_doc_link_in(index, doc_scope, owner_unit, &name, ctx.as_ref()).is_some()
            || is_first_party(&name)
            || is_escaped(text, span)
        {
            continue;
        }
        let own_parts = ctx
            .as_ref()
            .map_or_else(Vec::new, |c| own_part_names(index, c));
        let suggestions = suggestions(index, doc_scope, owner_unit, &name, &own_parts);
        // A lowercase single word with no near match (`[x]`, `[i]`, `[optional]`)
        // is far more likely prose or an index than a broken link; hint it only
        // when it looks like a misspelt fn or field.
        if suggestions.is_empty()
            && !name.contains('.')
            && name.starts_with(|c: char| c.is_ascii_lowercase())
        {
            continue;
        }
        // The name's own range inside the brackets (the label of a
        // `[text][Name]` is the trailing occurrence), for the quick fix.
        let candidate = &text[span.start..span.end];
        let name_start = span.start + candidate.rfind(name.as_str()).unwrap_or(0);
        let name_range: Range =
            crate::position::span_to_range(text, Span::new(name_start, name_start + name.len()));
        let message = match suggestions.first() {
            Some(best) => format!("Unresolved doc link `{name}`; did you mean `{best}`?"),
            None => format!(
                "Unresolved doc link `{name}`; escape it as `\\[{name}]` if the brackets are intentional"
            ),
        };
        out.push(Diagnostic {
            range: crate::position::span_to_range(text, span),
            severity: Some(severity),
            code: Some(NumberOrString::String(CODE.to_string())),
            source: Some("bynk".to_string()),
            message,
            data: Some(
                json!({ "name": name, "suggestions": suggestions, "nameRange": name_range }),
            ),
            ..Default::default()
        });
    }
    out
}

/// The did-you-mean quick-fix edits for one published diagnostic, as
/// `(suggestion, range to replace)`. Empty for any other diagnostic.
pub fn quick_fix_edits(diag: &Diagnostic) -> Vec<(String, Range)> {
    quick_fix_edits_in(diag, None)
}

/// As [`quick_fix_edits`], but when `text` is given the fix is offered only if
/// `text` still holds the unresolved name at the diagnostic's recorded range.
/// A client does not shift a published diagnostic's range as the user types, so
/// a range from an older round may now point at other characters.
fn quick_fix_edits_in(diag: &Diagnostic, text: Option<&str>) -> Vec<(String, Range)> {
    if diag.code != Some(NumberOrString::String(CODE.to_string())) {
        return Vec::new();
    }
    let Some(data) = &diag.data else {
        return Vec::new();
    };
    let Some(range) = data
        .get("nameRange")
        .and_then(|r| serde_json::from_value::<Range>(r.clone()).ok())
    else {
        return Vec::new();
    };
    if let Some(text) = text {
        let at = |p| crate::position::position_to_offset(text, p);
        let current = at(range.start)
            .zip(at(range.end))
            .and_then(|(start, end)| text.get(start..end));
        if current != data.get("name").and_then(|n| n.as_str()) {
            return Vec::new();
        }
    }
    data.get("suggestions")
        .and_then(|s| s.as_array())
        .map(|names| {
            names
                .iter()
                .filter_map(|n| n.as_str())
                .map(|n| (n.to_string(), range))
                .collect()
        })
        .unwrap_or_default()
}

/// The did-you-mean quick fixes for the unresolved-link diagnostics the client
/// sent with a `codeAction` request (`context.diagnostics`) that intersect
/// `requested`, against `text`, the analysed snapshot the edits are versioned to.
pub fn quick_fix_actions(
    text: &str,
    diagnostics: &[Diagnostic],
    requested: Range,
    uri: &Url,
    version: Option<i32>,
) -> Vec<CodeActionOrCommand> {
    let touches = |r: &Range| r.start <= requested.end && requested.start <= r.end;
    diagnostics
        .iter()
        .filter(|d| touches(&d.range))
        .flat_map(|d| {
            quick_fix_edits_in(d, Some(text))
                .into_iter()
                .map(move |(name, range)| {
                    CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Change to `{name}`"),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![d.clone()]),
                        edit: Some(WorkspaceEdit {
                            changes: None,
                            document_changes: Some(DocumentChanges::Edits(vec![
                                TextDocumentEdit {
                                    text_document: OptionalVersionedTextDocumentIdentifier {
                                        uri: uri.clone(),
                                        version,
                                    },
                                    edits: vec![OneOf::Left(TextEdit {
                                        range,
                                        new_text: name,
                                    })],
                                },
                            ])),
                            change_annotations: None,
                        }),
                        ..Default::default()
                    })
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_check::index::{SiteRef, SymbolEntry, SymbolKey, SymbolKind};
    use bynk_check::locals::LocalKind;
    use std::path::PathBuf;

    fn index() -> ProjectIndex {
        let mut index = ProjectIndex::default();
        for (kind, name) in [
            (SymbolKind::Agent, "Limiter"),
            (SymbolKind::Type, "Window"),
            (SymbolKind::Field, "Window.size"),
            (SymbolKind::Fn, "decide"),
        ] {
            index.symbols.insert(
                SymbolKey {
                    unit: "ratelimit".into(),
                    kind,
                    name: name.into(),
                },
                SymbolEntry {
                    def: Some(SiteRef {
                        path: PathBuf::from("ratelimit.bynk"),
                        span: Span::new(0, 1),
                    }),
                    ..Default::default()
                },
            );
        }
        index
    }

    fn scope() -> HashMap<String, Vec<String>> {
        HashMap::from([("ratelimit".to_string(), vec!["ratelimit".to_string()])])
    }

    fn doc_text(doc: &str) -> String {
        format!("--| {doc}\nfn f() -> Int {{ 1 }}\n")
    }

    fn run(doc: &str, level: Level) -> Vec<Diagnostic> {
        let text = doc_text(doc);
        unresolved_link_diagnostics(
            &text,
            Path::new("ratelimit.bynk"),
            "ratelimit",
            &index(),
            &scope(),
            &[],
            level,
        )
    }

    /// Run the hint on `text`, whose first declaration name is `owner`, with
    /// the index's `owner` entry (and any `extra` entries) defined in the file
    /// and `params` bound as that declaration's parameters.
    fn run_attached(
        text: &str,
        owner: (SymbolKind, &str),
        extra: &[(SymbolKind, &str)],
        params: &[&str],
    ) -> Vec<Diagnostic> {
        let mut index = index();
        let attached = text.find(owner.1).expect("owner in text");
        let mut define = |kind, name: &str, at: usize| {
            index.symbols.insert(
                SymbolKey {
                    unit: "ratelimit".into(),
                    kind,
                    name: name.into(),
                },
                SymbolEntry {
                    def: Some(SiteRef {
                        path: PathBuf::from("ratelimit.bynk"),
                        span: Span::new(at, at + 1),
                    }),
                    ..Default::default()
                },
            );
        };
        define(owner.0, owner.1, attached);
        for &(kind, name) in extra {
            define(kind, name, 0);
        }
        let locals: Vec<LocalBinding> = params
            .iter()
            .map(|p| {
                let at = text.find(&format!("{p}:")).expect("param in text");
                LocalBinding {
                    name: p.to_string(),
                    def_span: Span::new(at, at + p.len()),
                    kind: LocalKind::Param,
                    ty: "Int".into(),
                    scope: Span::new(at, text.len()),
                }
            })
            .collect();
        unresolved_link_diagnostics(
            text,
            Path::new("ratelimit.bynk"),
            "ratelimit",
            &index,
            &scope(),
            &locals,
            Level::Hint,
        )
    }

    #[test]
    fn a_field_link_in_its_own_records_doc_is_quiet() {
        // `[sizes]` is a sibling fn, so `[size]` is a near miss for it: were the
        // own part not resolved, the hint would fire with `sizes` suggested.
        let text = "--| The [size] of the window; see [sizes].\ntype Box { size: Int }\n";
        let diags = run_attached(
            text,
            (SymbolKind::Type, "Box"),
            &[(SymbolKind::Field, "Box.size"), (SymbolKind::Fn, "sizes")],
            &[],
        );
        assert!(diags.is_empty(), "{diags:?}");
        // The same text, with no own parts in the index, does hint.
        let diags = run_attached(
            text,
            (SymbolKind::Type, "Box"),
            &[(SymbolKind::Fn, "sizes")],
            &[],
        );
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn a_parameter_link_in_its_fns_doc_is_quiet() {
        let text =
            "--| Decides for [order], not [orders].\nfn place(order: Int) -> Int { order }\n";
        let diags = run_attached(
            text,
            (SymbolKind::Fn, "place"),
            &[(SymbolKind::Fn, "orders")],
            &["order"],
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn an_own_part_does_not_excuse_a_similar_unresolved_sibling() {
        let text = "--| Takes [order] and [ordre].\nfn place(order: Int) -> Int { order }\n";
        let diags = run_attached(text, (SymbolKind::Fn, "place"), &[], &["order"]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`ordre`"), "{}", diags[0].message);
    }

    #[test]
    fn an_ambiguous_own_part_is_not_suggested() {
        // `Size` is defined as both a field and a method of `Box`, so
        // `[Size]` does not resolve and `[Sizw]` must not be sent to it.
        let text = "--| The [Sizw].\ntype Box { Size: Int }\n";
        let diags = run_attached(
            text,
            (SymbolKind::Type, "Box"),
            &[
                (SymbolKind::Field, "Box.Size"),
                (SymbolKind::Method, "Box.Size"),
            ],
            &[],
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(quick_fix_edits(&diags[0]).is_empty());
    }

    #[test]
    fn the_hint_is_wired_to_the_analysed_locals() {
        // A real analysis round, not a hand-built index: the parameter comes
        // from `result.locals`, keyed by the file's project-relative path.
        let src = "commons pay\n\n--| Pays the [amount] and [amoutn].\nfn pay(amount: Int) -> Int { amount }\n";
        let root = std::env::temp_dir().join(format!("bynk-lsp-doclink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("pay.bynk"), src).unwrap();
        let root = root.canonicalize().unwrap();
        let overlay = HashMap::from([(root.join("pay.bynk"), src.to_string())]);
        let result = bynk_ide::diagnose_project(&root, &overlay);
        let _ = std::fs::remove_dir_all(&root);
        let file = &result.files[0];
        let diags = unresolved_link_diagnostics(
            &file.text,
            &file.source_path,
            "pay",
            &result.index,
            &result.doc_scope,
            result
                .locals
                .get(&file.source_path)
                .map_or(&[], Vec::as_slice),
            Level::Hint,
        );
        // `[amount]` resolves to the parameter; only the typo hints, and is
        // fixed to the parameter.
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(quick_fix_edits(&diags[0])[0].0, "amount");
    }

    #[test]
    fn did_you_mean_offers_own_parts() {
        let text = "--| Takes [ordr].\nfn place(order: Int) -> Int { order }\n";
        let diags = run_attached(text, (SymbolKind::Fn, "place"), &[], &["order"]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(quick_fix_edits(&diags[0])[0].0, "order");
        let text = "--| The [sizw].\ntype Box { size: Int }\n";
        let diags = run_attached(
            text,
            (SymbolKind::Type, "Box"),
            &[(SymbolKind::Field, "Box.size")],
            &[],
        );
        assert_eq!(quick_fix_edits(&diags[0])[0].0, "size");
    }

    #[test]
    fn a_misspelt_link_hints_with_a_did_you_mean() {
        let diags = run("See [Limtier].", Level::Hint);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::HINT));
        assert_eq!(d.code, Some(NumberOrString::String(CODE.into())));
        assert!(
            d.message.contains("did you mean `Limiter`?"),
            "{}",
            d.message
        );
        let fixes = quick_fix_edits(d);
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].0, "Limiter");
        // The edit replaces the name only, not the brackets: `--| See [` is 9 chars.
        assert_eq!(fixes[0].1.start.character, 9);
        assert_eq!(fixes[0].1.end.character, 16);
    }

    #[test]
    fn a_resolved_link_is_quiet() {
        assert!(run("See [Limiter] and [Window.size].", Level::Hint).is_empty());
    }

    #[test]
    fn a_name_with_no_near_match_hints_without_a_suggestion() {
        let diags = run("See [TODO].", Level::Hint);
        assert_eq!(diags.len(), 1);
        assert!(quick_fix_edits(&diags[0]).is_empty());
        assert!(diags[0].message.contains("escape it"));
    }

    #[test]
    fn an_escaped_bracket_is_quiet() {
        assert!(run("Still \\[TODO] here.", Level::Hint).is_empty());
    }

    #[test]
    fn a_bracket_inside_a_code_span_is_quiet() {
        assert!(run("Write `[TODO]` here.", Level::Hint).is_empty());
    }

    #[test]
    fn a_full_reference_fixes_its_label_not_its_text() {
        let diags = run("See [the limiter][Limtier].", Level::Hint);
        assert_eq!(diags.len(), 1);
        let (name, range) = &quick_fix_edits(&diags[0])[0];
        assert_eq!(name, "Limiter");
        // `--| See [the limiter][` is 22 chars.
        assert_eq!(range.start.character, 22);
    }

    #[test]
    fn the_setting_picks_the_severity_and_off_suppresses_all() {
        assert_eq!(
            run("[Nope]", Level::Warning)[0].severity,
            Some(DiagnosticSeverity::WARNING)
        );
        assert!(run("[Nope]", Level::Off).is_empty());
    }

    #[test]
    fn first_party_names_are_exempt() {
        // `Clock` is a first-party capability; its members don't resolve to a
        // def in the index today, so without the exemption each would hint.
        assert!(run("See [Clock.now] and [Clock].", Level::Hint).is_empty());
        assert!(is_first_party("Clock.now"));
        assert!(!is_first_party("Limiter"));
    }

    #[test]
    fn the_setting_is_read_from_either_payload_shape() {
        let nested = json!({ "bynk": { "docLinks": { "reportUnresolved": "warning" } } });
        let section = json!({ "docLinks": { "reportUnresolved": "off" } });
        assert_eq!(Level::from_settings(&nested), Some(Level::Warning));
        assert_eq!(Level::from_settings(&section), Some(Level::Off));
        assert_eq!(Level::from_settings(&json!({})), None);
        assert_eq!(
            Level::from_settings(&json!({ "docLinks": { "reportUnresolved": "loud" } })),
            None
        );
    }

    #[test]
    fn a_code_action_edits_the_diagnostic_range() {
        let diags = run("See [Limtier].", Level::Hint);
        let uri = Url::parse("file:///tmp/a.bynk").unwrap();
        let actions = quick_fix_actions(
            &doc_text("See [Limtier]."),
            &diags,
            diags[0].range,
            &uri,
            Some(3),
        );
        assert_eq!(actions.len(), 1);
        let CodeActionOrCommand::CodeAction(a) = &actions[0] else {
            panic!("a code action")
        };
        assert_eq!(a.title, "Change to `Limiter`");
        // A request elsewhere on the line offers nothing.
        let moved = Range {
            start: tower_lsp::lsp_types::Position::new(5, 0),
            end: tower_lsp::lsp_types::Position::new(5, 1),
        };
        assert!(
            quick_fix_actions(&doc_text("See [Limtier]."), &diags, moved, &uri, None).is_empty()
        );
    }

    #[test]
    fn lowercase_prose_brackets_are_quiet_but_a_misspelt_fn_is_not() {
        assert!(run("Done: [x] and a[i] and [optional].", Level::Hint).is_empty());
        let diags = run("See [decidee].", Level::Hint);
        assert_eq!(diags.len(), 1);
        assert_eq!(quick_fix_edits(&diags[0])[0].0, "decide");
        // A dotted lowercase name still hints: it is clearly meant as a link.
        assert_eq!(run("See [window.size].", Level::Hint).len(), 1);
    }

    #[test]
    fn a_fix_is_withheld_when_the_range_no_longer_holds_the_name() {
        let diags = run("See [Limtier].", Level::Hint);
        let uri = Url::parse("file:///tmp/a.bynk").unwrap();
        // The buffer the action sees has a line inserted above: the stale
        // range now covers other text, so no edit may be offered.
        let shifted = format!("--| intro text here\n{}", doc_text("See [Limtier]."));
        assert!(quick_fix_actions(&shifted, &diags, diags[0].range, &uri, None).is_empty());
        assert_eq!(
            quick_fix_actions(
                &doc_text("See [Limtier]."),
                &diags,
                diags[0].range,
                &uri,
                None
            )
            .len(),
            1
        );
    }
}
