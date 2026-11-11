//! v0.30.2 (ADR 0063): the expression-type sink.
//!
//! The checker computes `expr_types: HashMap<ExprId, TypedExpr>` per file as
//! it types each expression (T3.4, R2.4 — keyed by node identity, not
//! position). On the Ok path that map rides inside the `TypedCommons`; on the
//! error path inside `RecordCheck::typed_despite_errors`, the program as
//! checked so far.
//! This sink carries it out to the analysis so completion can ask *"what is
//! the type of the expression at this offset?"* (the receiver before a `.`),
//! mirroring [`HintSink`](crate::hints::HintSink).
//!
//! Capture is at **every per-file exit** of the check, clean or not (ADR 0094,
//! via [`record_analyse_types`](crate::check_pipeline::record_analyse_types)):
//! a mid-edit file with an error still yields the best-effort types of its
//! other expressions. ADR 0094 lifted the slice-3 "clean-file ceiling" of
//! ADR 0063, under which a file with errors yielded nothing.
//! Unlike hints, **test/integration files are not muted** (completion runs in
//! them); only synthetic toolchain-injected files are.

use crate::checker::{TyId, TypedExpr, Types};
use bynk_syntax::ast::ExprId;
use bynk_syntax::span::Span;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Project-relative source path → that file's `(expr span, type)` entries,
/// ordered by span (innermost-last within a start, so a containment search can
/// prefer the tightest match).
pub type FileExprTypes = HashMap<PathBuf, Vec<(Span, TyId)>>;

/// Records per-file expression types. A fresh sink records nothing until
/// [`enter_file`](Self::enter_file) attributes it.
#[derive(Debug, Default, Clone)]
pub struct ExprTypeSink {
    files: FileExprTypes,
    file: Option<PathBuf>,
    /// Set for synthetic (toolchain-injected) files — their types never serve
    /// a user-visible completion.
    muted: bool,
}

impl ExprTypeSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter a per-file recording context.
    pub fn enter_file(&mut self, file: &Path, muted: bool) {
        self.file = Some(file.to_path_buf());
        self.muted = muted;
    }

    /// Record a whole file's `expr_types` map — the Ok path's, or the error
    /// path's best-effort partial one (ADR 0094). Dropped when muted or before
    /// any `enter_file`.
    ///
    /// T3.4: the checker's own map is keyed by [`ExprId`] (R2.4) — position
    /// is never identity there. This sink's own storage stays position-keyed
    /// on purpose: an editor asks "what's at this cursor offset," a
    /// position-shaped question asked at the LSP boundary, not the
    /// checker's. `TypedExpr` carries the span the checker computed it
    /// against, so the join needs no separate id→span table.
    pub fn record_file(&mut self, expr_types: &HashMap<ExprId, TypedExpr>) {
        if self.muted {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let entry = self.files.entry(file.clone()).or_default();
        entry.extend(expr_types.values().map(|te| (te.span, te.ty)));
    }

    /// Fold in what `other` recorded (#3243 S2.3), per file, in order.
    pub fn absorb(&mut self, other: ExprTypeSink) {
        for (file, entries) in other.files {
            self.files.entry(file).or_default().extend(entries);
        }
    }

    /// Drain the recorded types, each file's entries ordered by span (start
    /// ascending, then **widest first** so a forward scan ends on the tightest
    /// containing span).
    pub fn take_files(&mut self) -> FileExprTypes {
        let mut files = std::mem::take(&mut self.files);
        for entries in files.values_mut() {
            entries.sort_by_key(|(span, _)| (span.start, std::cmp::Reverse(span.end)));
        }
        files
    }
}

/// The type of the **innermost** expression whose span contains `offset`, if
/// any — the receiver-typing query for `.`-member completion.
///
/// `None` when that innermost expression failed to type (its entry is
/// [`Ty::Error`](crate::checker::Ty::Error)). The search does not skip past it
/// to an enclosing expression: that is a *different* expression, and its type
/// would answer the question confidently wrong.
///
/// The innermost span's type is read through [`type_at_span`], so entries
/// that disagree at that exact position also answer `None`.
pub fn type_at_offset(entries: &[(Span, TyId)], offset: usize, tys: &Types) -> Option<TyId> {
    let (innermost, _) = entries
        .iter()
        .filter(|(span, _)| span.start <= offset && offset <= span.end)
        .min_by_key(|(span, _)| span.end - span.start)?;
    type_at_span(entries, *innermost).filter(|ty| !ty.is_error(tys))
}

/// The type recorded for the expression at exactly `span`'s position, or
/// `None` if nothing is recorded there or the entries there disagree.
///
/// Compares `start`/`end` only, never the `FileId`: an editor reparses the
/// buffer into spans with no file identity, while these entries carry the
/// file's real one (T3.5). Several entries can share one position (the same
/// expression recorded more than once, or two nodes sharing a source span),
/// and their order follows the checker's `HashMap`, so picking the first
/// would vary from run to run. Disagreeing entries are ambiguous, and
/// ambiguity answers "no type" rather than an arbitrary one.
pub fn type_at_span(entries: &[(Span, TyId)], span: Span) -> Option<TyId> {
    let mut at = entries
        .iter()
        .filter(|(s, _)| s.start == span.start && s.end == span.end)
        .map(|(_, ty)| *ty);
    let first = at.next()?;
    at.all(|ty| ty == first).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::{Ty, Types};
    use bynk_syntax::ast::BaseType;

    fn span(start: usize, end: usize) -> Span {
        Span::new(start, end)
    }

    #[test]
    fn type_at_offset_prefers_the_innermost_span() {
        let tys = Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let string = tys.intern(Ty::Base(BaseType::String));
        // An outer `String` expression 0..10 with an inner `Int` 2..4.
        let entries = vec![(span(0, 10), string), (span(2, 4), int)];
        assert_eq!(type_at_offset(&entries, 3, &tys), Some(int)); // inside the inner span
        assert_eq!(type_at_offset(&entries, 7, &tys), Some(string)); // outer span only
        assert_eq!(type_at_offset(&entries, 20, &tys), None); // outside everything
    }

    #[test]
    fn disagreeing_entries_at_one_position_are_no_type() {
        let tys = Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let string = tys.intern(Ty::Base(BaseType::String));
        // A repeated identical entry is still one answer...
        let agreeing = vec![(span(0, 10), int), (span(0, 10), int)];
        assert_eq!(type_at_span(&agreeing, span(0, 10)), Some(int));
        assert_eq!(type_at_offset(&agreeing, 3, &tys), Some(int));
        // ...but two types recorded at one position are ambiguous, in
        // either order.
        for entries in [
            vec![(span(0, 10), int), (span(0, 10), string)],
            vec![(span(0, 10), string), (span(0, 10), int)],
        ] {
            assert_eq!(type_at_span(&entries, span(0, 10)), None);
            assert_eq!(type_at_offset(&entries, 3, &tys), None);
        }
    }

    #[test]
    fn an_error_typed_innermost_span_is_no_type_not_its_parent() {
        let tys = Types::new();
        let string = tys.intern(Ty::Base(BaseType::String));
        let error = tys.intern(Ty::Error);
        // A well-typed outer expression 0..10 around an inner one 2..4 whose
        // typing failed: the inner offset has no type, rather than the
        // outer expression's.
        let entries = vec![(span(0, 10), string), (span(2, 4), error)];
        assert_eq!(type_at_offset(&entries, 3, &tys), None);
        assert_eq!(type_at_offset(&entries, 7, &tys), Some(string));
    }

    /// Each expression is recorded once per file. A file that fails
    /// `check_record` but keeps being checked (#1663) used to be recorded at
    /// the `Err` exit and again at the final `failed` exit, doubling every
    /// entry.
    ///
    /// The assertion is that no two entries share a span, which is stronger
    /// than "recorded once": the checker does not guarantee that two typed
    /// nodes never share a span (see `check_record_in`'s finding #28, bug
    /// #844 and the else-less `if`). A fixture here must avoid such pairs.
    #[test]
    fn each_expression_is_recorded_once() {
        for src in [
            "context c\n\nfn f(num: Int) -> Int {\n  num * 2\n}\n",
            "context c\n\nfn f(num: Int) -> Int {\n  num + \"s\"\n}\n",
            // A record error, then a handler body typed by the declaration
            // checks, which also fails.
            "context c\n\nfn f(num: Int) -> Int {\n  num + \"s\"\n}\n\n\
             service api from http {\n  on GET(\"/x\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(1 + \"s\")\n  }\n}\n",
        ] {
            let a = crate::testkit::analyse(&[("c.bynk", src)]);
            let (_, entries) = a
                .analysis
                .expr_types
                .iter()
                .find(|(path, _)| path.ends_with("c.bynk"))
                .expect("types recorded for c.bynk");
            let mut spans: Vec<_> = entries
                .iter()
                .map(|(span, _)| (span.start, span.end))
                .collect();
            assert!(!spans.is_empty(), "no types recorded for {src:?}");
            let total = spans.len();
            spans.dedup();
            assert_eq!(
                spans.len(),
                total,
                "duplicate spans for {src:?}: {entries:?}"
            );
            if src.contains("service") {
                // The handler body was typed and recorded, not just `f`.
                assert_eq!(a.type_at("c.bynk", "1"), "Int");
            }
        }
    }
}
