//! Selecting and applying the machine-applicable suggestions of a file's
//! diagnostics — the rule behind the editor's `source.fixAll.bynk` (#2100
//! slice 6) and `bynk check --fix` (#2642), which share it so the two cannot
//! drift.
//!
//! DECISION C (#2100): only a suggestion marked
//! [`Applicability::MachineApplicable`] is taken, and only from a diagnostic
//! that carries exactly one such suggestion. A diagnostic offering several
//! (a choice between candidates) needs a human, so it stays an individual
//! quick fix.
//!
//! Suggestions are taken in diagnostic order. A suggestion whose edits overlap
//! an edit already taken is skipped whole, never applied in part. Two
//! insertions at the same point overlap unless they are identical, so their
//! order never matters. An identical edit is taken once (two diagnostics
//! asking for the same `given` entry add it once). A skipped suggestion is
//! still a quick fix, and the next fix-all picks it up once the first batch is
//! applied.

use crate::error::{Applicability, CompileError};
use crate::span::Span;

/// The one machine-applicable suggestion `error` carries, if it carries
/// exactly one. `None` for no fix, a placeholder fix, or a choice between
/// candidates.
pub fn lone_fix(error: &CompileError) -> Option<&crate::error::Suggestion> {
    let mut applicable = error
        .suggestions
        .iter()
        .filter(|s| s.applicability == Applicability::MachineApplicable);
    match (applicable.next(), applicable.next()) {
        (Some(s), None) => Some(s),
        _ => None,
    }
}

/// What [`select`] chose: the edits, and the category of each diagnostic whose
/// suggestion contributed at least one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Sorted by position and non-overlapping.
    pub edits: Vec<(Span, String)>,
    pub categories: Vec<&'static str>,
}

/// Every edit fix-all would apply to `text`, the file whose `errors` these
/// are, sorted by position and non-overlapping. Empty when there is nothing to
/// fix. A suggestion with an edit span that isn't valid for `text` (out of
/// bounds, reversed, or off a char boundary) is skipped whole.
pub fn edits<'a>(
    text: &str,
    errors: impl IntoIterator<Item = &'a CompileError>,
) -> Vec<(Span, String)> {
    select(text, errors).edits
}

/// [`edits`], also naming the diagnostics the edits came from.
pub fn select<'a>(text: &str, errors: impl IntoIterator<Item = &'a CompileError>) -> Selection {
    let mut taken: Vec<(Span, String)> = Vec::new();
    let mut categories = Vec::new();
    for e in errors {
        let Some(s) = lone_fix(e) else {
            continue; // none, or a choice between candidates
        };
        if !s.edits.iter().all(|(span, _)| valid(text, *span)) {
            continue;
        }
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
        if !fresh.is_empty() {
            categories.push(e.category);
        }
        taken.extend(fresh.into_iter().cloned());
    }
    taken.sort_by_key(|(span, _)| (span.start, span.end));
    Selection {
        edits: taken,
        categories,
    }
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

/// `span` is a range of `text`: in bounds, not reversed, on char boundaries.
fn valid(text: &str, span: Span) -> bool {
    span.start <= span.end && text.get(span.start..span.end).is_some()
}

/// The text with `edits` (sorted, non-overlapping) applied. An edit whose
/// span isn't a valid range of `text`, or that overlaps the one before it, is
/// skipped rather than panicking.
pub fn apply(text: &str, edits: &[(Span, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (span, new_text) in edits {
        let Some(before) = text.get(last..span.start) else {
            continue;
        };
        if !valid(text, *span) {
            continue;
        }
        out.push_str(before);
        out.push_str(new_text);
        last = span.end;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One suggestion: its `((start, end), replacement)` edits and applicability.
    type Fix<'a> = (&'a [((usize, usize), &'a str)], Applicability);

    fn diag(span: (usize, usize), fixes: &[Fix]) -> CompileError {
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
        error
    }

    /// Long enough for every span the tests below use.
    const TEXT: &str = "0123456789012345678901234567890123456789";
    const M: Applicability = Applicability::MachineApplicable;
    const P: Applicability = Applicability::HasPlaceholders;

    #[test]
    fn takes_every_lone_machine_applicable_suggestion_in_position_order() {
        let ds = [
            diag((10, 12), &[(&[((10, 12), "b")], M)]),
            diag((0, 2), &[(&[((0, 2), "a")], M)]),
        ];
        assert_eq!(
            edits(TEXT, &ds),
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
        assert_eq!(edits(TEXT, &ds), vec![(Span::new(8, 9), "z".to_string())]);
    }

    #[test]
    fn an_overlapping_suggestion_is_skipped_whole() {
        let ds = [
            diag((0, 4), &[(&[((0, 4), "a")], M)]),
            // Its second edit overlaps the first suggestion's: neither of its
            // edits is taken.
            diag((10, 12), &[(&[((10, 12), "b"), ((2, 3), "c")], M)]),
        ];
        assert_eq!(edits(TEXT, &ds), vec![(Span::new(0, 4), "a".to_string())]);
    }

    #[test]
    fn an_identical_edit_is_taken_once() {
        // Two uses of an undeclared capability both ask to add it to the same
        // `given` clause.
        let ds = [
            diag((20, 24), &[(&[((14, 14), ", Used")], M)]),
            diag((30, 34), &[(&[((14, 14), ", Used")], M)]),
        ];
        assert_eq!(
            edits(TEXT, &ds),
            vec![(Span::new(14, 14), ", Used".to_string())]
        );
    }

    #[test]
    fn different_insertions_at_one_point_conflict() {
        let ds = [
            diag((20, 24), &[(&[((14, 14), ", A")], M)]),
            diag((30, 34), &[(&[((14, 14), ", B")], M)]),
        ];
        assert_eq!(
            edits(TEXT, &ds),
            vec![(Span::new(14, 14), ", A".to_string())]
        );
    }

    #[test]
    fn touching_edits_do_not_conflict() {
        let ds = [
            diag((0, 2), &[(&[((0, 2), "a")], M)]),
            diag((2, 4), &[(&[((2, 4), "b")], M)]),
            diag((4, 4), &[(&[((4, 4), "c")], M)]),
        ];
        assert_eq!(edits(TEXT, &ds).len(), 3);
        assert_eq!(apply("0123456", &edits(TEXT, &ds)), "abc456");
    }

    #[test]
    fn an_insertion_inside_a_replacement_conflicts() {
        let ds = [
            diag((0, 4), &[(&[((0, 4), "a")], M)]),
            diag((2, 2), &[(&[((2, 2), "b")], M)]),
        ];
        assert_eq!(edits(TEXT, &ds), vec![(Span::new(0, 4), "a".to_string())]);
    }

    #[test]
    fn a_suggestion_with_a_bad_span_is_skipped() {
        let ds = [
            // Past the end of the text.
            diag((0, 1), &[(&[((38, 99), "x")], M)]),
            // Reversed.
            diag((0, 1), &[(&[((5, 3), "y")], M)]),
            diag((0, 1), &[(&[((0, 1), "z")], M)]),
        ];
        assert_eq!(edits(TEXT, &ds), vec![(Span::new(0, 1), "z".to_string())]);
        // Off a char boundary: inside the two-byte `é`.
        let ds = [diag((0, 1), &[(&[((2, 2), "w")], M)])];
        assert!(edits("aé", &ds).is_empty());
    }

    #[test]
    fn apply_skips_a_bad_span_instead_of_panicking() {
        let bad = [
            (Span::new(0, 1), "A".to_string()),
            (Span::new(2, 99), "!".to_string()),
        ];
        assert_eq!(apply("abc", &bad), "Abc");
        assert_eq!(apply("aé", &[(Span::new(2, 2), "w".to_string())]), "aé");
    }

    #[test]
    fn nothing_to_fix_is_empty() {
        assert!(edits(TEXT, &[] as &[CompileError]).is_empty());
        assert!(edits(TEXT, &[diag((0, 1), &[])]).is_empty());
    }

    #[test]
    fn select_names_the_diagnostics_it_took_from() {
        let ds = [
            diag((0, 2), &[(&[((0, 2), "a")], M)]),
            // Skipped: overlaps the first.
            diag((1, 3), &[(&[((1, 3), "b")], M)]),
            diag((10, 12), &[(&[((10, 12), "c")], M)]),
        ];
        let s = select(TEXT, &ds);
        assert_eq!(s.edits.len(), 2);
        assert_eq!(s.categories, vec!["bynk.test", "bynk.test"]);
    }

    #[test]
    fn lone_fix_is_only_a_single_machine_applicable_suggestion() {
        assert!(lone_fix(&diag((0, 1), &[(&[((0, 1), "a")], M)])).is_some());
        assert!(lone_fix(&diag((0, 1), &[(&[((0, 1), "a")], P)])).is_none());
        assert!(
            lone_fix(&diag(
                (0, 1),
                &[(&[((0, 1), "a")], M), (&[((0, 1), "b")], M)]
            ))
            .is_none()
        );
        assert!(lone_fix(&diag((0, 1), &[])).is_none());
    }
}
