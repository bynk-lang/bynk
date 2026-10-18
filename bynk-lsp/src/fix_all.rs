//! #2100 slice 6: the `source.fixAll.bynk` code action — apply every
//! machine-applicable diagnostic suggestion in a file as one edit.
//!
//! DECISION C: only a suggestion marked
//! [`Applicability::MachineApplicable`] is taken, and only from a diagnostic
//! that carries exactly one such suggestion. A diagnostic offering several
//! (a choice between candidates) needs a human, so it stays an individual
//! quick fix. The suggestions ride on the committed round's diagnostics, the
//! same ones [`crate::code_actions::quick_fixes`] offers one at a time.
//!
//! Suggestions are taken in diagnostic order. A suggestion whose edits overlap
//! an edit already taken is skipped whole, never applied in part. Two
//! insertions at the same point overlap unless they are identical, so their
//! order never matters. An identical edit is taken once (two diagnostics
//! asking for the same `given` entry add it once). A skipped suggestion is
//! still a quick fix, and the next fix-all picks it up once the first batch is
//! applied.
//!
//! The pure computation lives here. The `code_action` handler adds DECISION
//! B's safety gate: the combined edit is offered only if the re-analysed
//! project has no diagnostic the round didn't already have.

use bynk_syntax::error::Applicability;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

/// The `source.fixAll.bynk` kind.
pub fn kind() -> CodeActionKind {
    CodeActionKind::new("source.fixAll.bynk")
}

/// Every edit fix-all would apply to the file whose `diagnostics` these are,
/// sorted by position and non-overlapping. Empty when there is nothing to fix.
pub fn edits(diagnostics: &[bynk_ide::Diagnostic]) -> Vec<(Span, String)> {
    let mut taken: Vec<(Span, String)> = Vec::new();
    for d in diagnostics {
        let mut applicable = d
            .error
            .suggestions
            .iter()
            .filter(|s| s.applicability == Applicability::MachineApplicable);
        let (Some(s), None) = (applicable.next(), applicable.next()) else {
            continue; // none, or a choice between candidates
        };
        let fresh: Vec<&(Span, String)> = s
            .edits
            .iter()
            .filter(|e| !taken.iter().any(|t| t.0 == e.0 && t.1 == e.1))
            .collect();
        if fresh
            .iter()
            .any(|e| taken.iter().any(|t| overlaps(t.0, e.0)))
            || fresh
                .iter()
                .enumerate()
                .any(|(i, a)| fresh[i + 1..].iter().any(|b| overlaps(a.0, b.0)))
        {
            continue;
        }
        taken.extend(fresh.into_iter().cloned());
    }
    taken.sort_by_key(|(span, _)| (span.start, span.end));
    taken
}

/// Two edit spans conflict: they share a character, or they are insertions
/// at the same point. Spans that only touch (one ends where the next starts)
/// don't, unless both are that same empty span.
fn overlaps(a: Span, b: Span) -> bool {
    if a.start == a.end && b.start == b.end {
        return a.start == b.start;
    }
    a.start < b.end && b.start < a.end
}

/// The text with `edits` (sorted, non-overlapping) applied.
pub fn apply(text: &str, edits: &[(Span, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (span, new_text) in edits {
        out.push_str(&text[last..span.start]);
        out.push_str(new_text);
        last = span.end;
    }
    out.push_str(&text[last..]);
    out
}

/// The versioned `source.fixAll.bynk` action for `edits`.
pub fn action(
    text: &str,
    edits: &[(Span, String)],
    uri: &Url,
    version: Option<i32>,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: "Fix all auto-fixable problems".to_string(),
        kind: Some(kind()),
        edit: Some(WorkspaceEdit {
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
        }),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::error::CompileError;

    /// One suggestion: its `((start, end), replacement)` edits and applicability.
    type Fix<'a> = (&'a [((usize, usize), &'a str)], Applicability);

    fn diag(span: (usize, usize), fixes: &[Fix]) -> bynk_ide::Diagnostic {
        let mut error = CompileError::new("bynk.test", Span::new(span.0, span.1), "msg");
        for (edits, applicability) in fixes {
            error = error.with_suggestion(
                "fix",
                edits
                    .iter()
                    .map(|((s, e), t)| (Span::new(*s, *e), t.to_string()))
                    .collect(),
                *applicability,
            );
        }
        bynk_ide::Diagnostic {
            severity: bynk_syntax::Severity::Error,
            label_files: Vec::new(),
            error,
        }
    }

    const M: Applicability = Applicability::MachineApplicable;
    const P: Applicability = Applicability::HasPlaceholders;

    #[test]
    fn takes_every_lone_machine_applicable_suggestion_in_position_order() {
        let ds = [
            diag((10, 12), &[(&[((10, 12), "b")], M)]),
            diag((0, 2), &[(&[((0, 2), "a")], M)]),
        ];
        assert_eq!(
            edits(&ds),
            vec![
                (Span::new(0, 2), "a".to_string()),
                (Span::new(10, 12), "b".to_string())
            ]
        );
    }

    #[test]
    fn skips_placeholders_and_choices() {
        let ds = [
            diag((0, 2), &[(&[((0, 2), "<T>")], P)]),
            // Two machine-applicable candidates: a human picks.
            diag((5, 6), &[(&[((5, 6), "x")], M), (&[((5, 6), "y")], M)]),
            // One machine-applicable beside a placeholder one: still lone.
            diag((8, 9), &[(&[((8, 9), "z")], M), (&[((8, 9), "<T>")], P)]),
        ];
        assert_eq!(edits(&ds), vec![(Span::new(8, 9), "z".to_string())]);
    }

    #[test]
    fn an_overlapping_suggestion_is_skipped_whole() {
        let ds = [
            diag((0, 4), &[(&[((0, 4), "a")], M)]),
            // Its second edit overlaps the first suggestion's: neither of its
            // edits is taken.
            diag((10, 12), &[(&[((10, 12), "b"), ((2, 3), "c")], M)]),
        ];
        assert_eq!(edits(&ds), vec![(Span::new(0, 4), "a".to_string())]);
    }

    #[test]
    fn an_identical_edit_is_taken_once() {
        // Two uses of an undeclared capability both ask to add it to the same
        // `given` clause.
        let ds = [
            diag((20, 24), &[(&[((14, 14), ", Used")], M)]),
            diag((30, 34), &[(&[((14, 14), ", Used")], M)]),
        ];
        assert_eq!(edits(&ds), vec![(Span::new(14, 14), ", Used".to_string())]);
    }

    #[test]
    fn different_insertions_at_one_point_conflict() {
        let ds = [
            diag((20, 24), &[(&[((14, 14), ", A")], M)]),
            diag((30, 34), &[(&[((14, 14), ", B")], M)]),
        ];
        assert_eq!(edits(&ds), vec![(Span::new(14, 14), ", A".to_string())]);
    }

    #[test]
    fn touching_edits_do_not_conflict() {
        let ds = [
            diag((0, 2), &[(&[((0, 2), "a")], M)]),
            diag((2, 4), &[(&[((2, 4), "b")], M)]),
            diag((4, 4), &[(&[((4, 4), "c")], M)]),
        ];
        assert_eq!(edits(&ds).len(), 3);
        assert_eq!(apply("0123456", &edits(&ds)), "abc456");
    }

    #[test]
    fn an_insertion_inside_a_replacement_conflicts() {
        let ds = [
            diag((0, 4), &[(&[((0, 4), "a")], M)]),
            diag((2, 2), &[(&[((2, 2), "b")], M)]),
        ];
        assert_eq!(edits(&ds), vec![(Span::new(0, 4), "a".to_string())]);
    }

    #[test]
    fn nothing_to_fix_is_empty() {
        assert!(edits(&[]).is_empty());
        assert!(edits(&[diag((0, 1), &[])]).is_empty());
    }
}
