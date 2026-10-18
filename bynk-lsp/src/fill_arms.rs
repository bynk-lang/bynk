//! #2100 slice 3: the fill-match-arms quick fix. It adds an arm for every
//! variant (or `Bool` value, or the `_` an unbounded literal match or an
//! extensible enum needs) that a non-exhaustive `match` leaves uncovered.
//!
//! The checker raises one `bynk.types.non_exhaustive_match` per missing
//! witness, and one `bynk.types.extensible_match_needs_wildcard` when the
//! enum may gain variants (its witness is the wildcard, `_` or `Some(_)`,
//! that covers them). Each carries a `HasPlaceholders` suggestion inserting
//! `\n<witness> => <default>` after the last arm, where `<default>` is a
//! typed default of the arm result type (`0`, `""`, `None`, `[]`, …). The
//! suggestion is never a one-click quick fix or a fix-all candidate (both take
//! only `MachineApplicable`). This module collects every suggestion on one
//! `match` into one action and lays the arms out the way `bynk-fmt` prints
//! them: one per line at the last arm's indentation, each ending in `,`.
//!
//! The action is offered only when **every** witness of the match has a
//! suggestion. A result type with no default (a record, a user sum, a refined
//! type), a `let … else match`, or a multi-field witness the checker can't
//! write as a pattern means no action. Filling only some of the arms would
//! still leave the match non-exhaustive.
//!
//! Fill-arms rides the unfiltered lightbulb request, so it's listed without
//! an edit (`data` names the match) and the edit is computed, and re-checked
//! (DECISION B), in `codeAction/resolve`.

use bynk_syntax::error::Applicability;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

/// The categories a missing arm is reported under: a missing variant (or
/// value), and the wildcard an extensible enum's future variants need.
const CATEGORIES: [&str; 2] = [
    "bynk.types.non_exhaustive_match",
    "bynk.types.extensible_match_needs_wildcard",
];
const MESSAGE_PREFIX: &str = "add a match arm for ";
const DATA_TAG: &str = "bynk.fillMatchArms";

/// The `match` spans (the non-exhaustive diagnostics' span) that intersect
/// `requested` and have an arm suggestion for every missing witness, in
/// source order.
pub fn fillable(diagnostics: &[bynk_ide::Diagnostic], requested: Span) -> Vec<Span> {
    let mut spans: Vec<Span> = diagnostics
        .iter()
        .filter(|d| CATEGORIES.contains(&d.error.category))
        .map(|d| d.error.span)
        .filter(|s| s.start <= requested.end && requested.start <= s.end)
        .collect();
    spans.sort_by_key(|s| (s.start, s.end));
    spans.dedup_by(|a, b| same_span(*a, *b));
    spans.retain(|s| arms(diagnostics, *s).is_some());
    spans
}

/// Spans compare by offsets: a span rebuilt from an action's `data` has no
/// file id.
fn same_span(a: Span, b: Span) -> bool {
    (a.start, a.end) == (b.start, b.end)
}

/// The `(anchor, arm text)` for every missing witness of the match at
/// `match_span`, or `None` unless every one of them has a suggestion and they
/// share one anchor.
fn arms(diagnostics: &[bynk_ide::Diagnostic], match_span: Span) -> Option<(usize, Vec<String>)> {
    let mut anchor = None;
    let mut out = Vec::new();
    for d in diagnostics
        .iter()
        .filter(|d| CATEGORIES.contains(&d.error.category) && same_span(d.error.span, match_span))
    {
        let s = d.error.suggestions.iter().find(|s| {
            s.applicability == Applicability::HasPlaceholders
                && s.message.starts_with(MESSAGE_PREFIX)
        })?;
        let [(span, text)] = s.edits.as_slice() else {
            return None;
        };
        if *anchor.get_or_insert(span.start) != span.start || span.start != span.end {
            return None;
        }
        let arm = text.trim_start_matches('\n').to_string();
        if !out.contains(&arm) {
            out.push(arm);
        }
    }
    Some((anchor?, out))
}

/// The edits that fill the match at `match_span` in `text`, laid out as
/// `bynk-fmt` prints arms. `None` when there is nothing to fill or the last
/// arm's line doesn't allow it: the match closes on that line, or the arm
/// shares it with other code. A `bynk-fmt`-canonical file stays canonical.
pub fn edits(
    text: &str,
    diagnostics: &[bynk_ide::Diagnostic],
    match_span: Span,
) -> Option<Vec<(Span, String)>> {
    let (anchor, arms) = arms(diagnostics, match_span)?;
    let line_start = text[..anchor].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[anchor..].find('\n').map_or(text.len(), |i| anchor + i);
    let indent: String = text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let rest = text[anchor..line_end].trim_start();
    let (has_comma, after) = match rest.strip_prefix(',') {
        Some(after) => (true, after.trim_start()),
        None => (false, rest),
    };
    if !after.is_empty() && !after.starts_with("--") {
        return None;
    }
    let mut out = Vec::new();
    if !has_comma {
        out.push((Span::new(anchor, anchor), ",".to_string()));
    }
    let inserted: String = arms.iter().map(|a| format!("\n{indent}{a},")).collect();
    out.push((Span::new(line_end, line_end), inserted));

    let filled = crate::fix_all::apply(text, &out);
    let opts = bynk_fmt::FormatOptions::default();
    let canonical = |s: &str| bynk_fmt::format_source(s, &opts).is_ok_and(|f| f == s);
    if canonical(text) && !canonical(&filled) {
        return None;
    }
    Some(out)
}

/// The listed (unresolved) action for the match at `match_span`: a quick fix
/// on its diagnostics, with no edit yet. `data` carries what
/// `codeAction/resolve` needs.
pub fn listed(
    diagnostics: &[bynk_ide::Diagnostic],
    match_span: Span,
    uri: &Url,
    version: Option<i32>,
) -> CodeActionOrCommand {
    // The arms actually inserted, so the title matches the edit.
    let count = arms(diagnostics, match_span).map_or(0, |(_, arms)| arms.len());
    CodeActionOrCommand::CodeAction(CodeAction {
        title: if count == 1 {
            "Fill the missing match arm".to_string()
        } else {
            format!("Fill {count} missing match arms")
        },
        kind: Some(CodeActionKind::QUICKFIX),
        data: Some(serde_json::json!({
            "kind": DATA_TAG,
            "uri": uri.as_str(),
            "version": version,
            "start": match_span.start,
            "end": match_span.end,
        })),
        ..Default::default()
    })
}

/// What a listed fill-arms action's `data` names, or `None` when the action
/// is not one of ours.
pub struct Target {
    pub uri: Url,
    pub version: Option<i32>,
    pub match_span: Span,
}

impl Target {
    pub fn of(action: &CodeAction) -> Option<Self> {
        let data = action.data.as_ref()?;
        if data.get("kind")?.as_str()? != DATA_TAG {
            return None;
        }
        Some(Target {
            uri: Url::parse(data.get("uri")?.as_str()?).ok()?,
            version: data
                .get("version")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok()),
            match_span: Span::new(
                usize::try_from(data.get("start")?.as_u64()?).ok()?,
                usize::try_from(data.get("end")?.as_u64()?).ok()?,
            ),
        })
    }
}

/// `action` with its versioned edit filled in.
pub fn resolve(
    mut action: CodeAction,
    text: &str,
    edits: &[(Span, String)],
    uri: &Url,
    version: Option<i32>,
) -> CodeAction {
    action.edit = Some(WorkspaceEdit {
        changes: None,
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version,
            },
            edits: edits
                .iter()
                .map(|(span, new_text)| {
                    OneOf::Left(TextEdit {
                        range: crate::position::span_to_range(text, *span),
                        new_text: new_text.clone(),
                    })
                })
                .collect(),
        }])),
        change_annotations: None,
    });
    action
}
