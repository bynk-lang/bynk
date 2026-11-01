//! #2100 slice 6: the `source.fixAll.bynk` code action — apply every
//! machine-applicable diagnostic suggestion in a file as one edit.
//!
//! The selection rule (DECISION C) lives in [`bynk_syntax::fix`], shared with
//! `bynk check --fix` (#2642); this module adapts it to the IDE's
//! [`bynk_ide::Diagnostic`] and builds the LSP action. The `code_action`
//! handler adds DECISION B's safety gate: the combined edit is offered only if
//! the re-analysed project has no diagnostic the round didn't already have.

use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

pub use bynk_syntax::fix::apply;

/// The `source.fixAll.bynk` kind.
pub fn kind() -> CodeActionKind {
    CodeActionKind::new("source.fixAll.bynk")
}

/// Every edit fix-all would apply to `text`, the file whose `diagnostics`
/// these are. See [`bynk_syntax::fix::edits`].
pub fn edits(text: &str, diagnostics: &[bynk_ide::Diagnostic]) -> Vec<(Span, String)> {
    bynk_syntax::fix::edits(text, diagnostics.iter().map(|d| &d.error))
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
    use bynk_syntax::error::{Applicability, CompileError};

    fn diag(span: (usize, usize), text: &str) -> bynk_ide::Diagnostic {
        let error = CompileError::new("bynk.test", Span::new(span.0, span.1), "msg")
            .with_suggestion(
                "fix",
                vec![(Span::new(span.0, span.1), text.to_string())],
                Applicability::MachineApplicable,
            );
        bynk_ide::Diagnostic {
            severity: bynk_syntax::Severity::Error,
            label_files: Vec::new(),
            error,
        }
    }

    /// Constrained generics slice 0: fix-all migrates a file that compares an
    /// unbounded type parameter in one action — `missing_eq_bound`'s suggestion
    /// is machine-applicable and the result compiles.
    #[test]
    fn migrates_missing_eq_bounds_in_one_action() {
        let text = "commons m\n\ntype Box[A] = { value: A }\n\nfn same[T](a: T, b: T) -> Bool { a == b }\n\nfn Box.has(self, x: A) -> Bool { self.value == x }\n";
        let diagnostics = bynk_ide::diagnose(text);
        let taken = edits(text, &diagnostics);
        assert_eq!(taken.len(), 2, "{taken:?}");
        let mut out = text.to_string();
        for (span, replacement) in taken.iter().rev() {
            out.replace_range(span.start..span.end, replacement);
        }
        assert!(out.contains("fn same[T: Eq](a: T, b: T)"), "{out}");
        assert!(
            out.contains("fn Box.has(self, x: A) -> Bool where A: Eq {"),
            "{out}"
        );
        assert!(bynk_ide::diagnose(&out).is_empty());
    }

    /// Apply fix-all's edits for `text`, returning the result.
    fn fixed(text: &str) -> String {
        let diagnostics = bynk_ide::diagnose(text);
        bynk_syntax::fix::apply(text, &edits(text, &diagnostics))
    }

    /// Constrained generics slice 1: fix-all adds `Ord` where a type parameter
    /// is ordered without it — a missing bound, a bound `Eq` upgraded to `Ord`
    /// (the edit replaces the `Eq`, leaving one bound), and a method's `where`.
    #[test]
    fn migrates_missing_ord_bounds_in_one_action() {
        let text = "commons m\n\ntype Box[A] = { value: A }\n\nfn lt[T](a: T, b: T) -> Bool { a < b }\n\nfn le[T: Eq](a: T, b: T) -> Bool { a <= b }\n\nfn Box.big(self, x: A) -> Bool { self.value > x }\n";
        let diagnostics = bynk_ide::diagnose(text);
        let cats: Vec<&str> = diagnostics.iter().map(|d| d.error.category).collect();
        assert_eq!(
            cats.iter()
                .filter(|c| **c == "bynk.generics.missing_ord_bound")
                .count(),
            3,
            "{cats:?}"
        );
        let out = fixed(text);
        assert!(out.contains("fn lt[T: Ord](a: T, b: T)"), "{out}");
        assert!(out.contains("fn le[T: Ord](a: T, b: T)"), "{out}");
        assert!(
            !out.contains("Eq"),
            "the Eq is replaced, not kept beside Ord: {out}"
        );
        assert!(
            out.contains("fn Box.big(self, x: A) -> Bool where A: Ord {"),
            "{out}"
        );
        assert!(bynk_ide::diagnose(&out).is_empty(), "{out}");
    }

    /// Constrained generics slice 2: fix-all adds `Key` where a type parameter
    /// keys a `Map` or `Set` without it, replaces a bound `Eq`, and drops the
    /// redundant `Eq` beside `Key`.
    #[test]
    fn migrates_missing_key_bounds_in_one_action() {
        let text = "commons m\n\nfn mk[K](k: K) -> Map[K, Int] { Map.empty() }\n\nfn st[K: Eq](k: K) -> Set[K] { Set.empty() }\n\nfn both[K: Eq + Key](k: K) -> Set[K] { Set.empty() }\n";
        let cats: Vec<&str> = bynk_ide::diagnose(text)
            .iter()
            .map(|d| d.error.category)
            .collect();
        assert!(
            cats.contains(&"bynk.generics.missing_key_bound"),
            "{cats:?}"
        );
        assert!(
            cats.contains(&"bynk.generics.redundant_eq_bound"),
            "{cats:?}"
        );
        let out = fixed(text);
        assert!(out.contains("fn mk[K: Key](k: K)"), "{out}");
        assert!(out.contains("fn st[K: Key](k: K)"), "{out}");
        assert!(out.contains("fn both[K: Key](k: K)"), "{out}");
        assert!(bynk_ide::diagnose(&out).is_empty(), "{out}");
    }

    /// Constrained generics slice 1: the redundant `Eq` beside `Ord`, and a
    /// `derives` of an automatic constraint, are removed by fix-all.
    #[test]
    fn removes_redundant_eq_and_automatic_derives() {
        let text = "commons m\n\nfn lt[T: Eq + Ord](a: T, b: T) -> Bool { a < b }\n\nfn gt[T: Ord + Eq](a: T, b: T) -> Bool { a > b }\n\ntype P = { a: Int } derives Eq\n\ntype Q = { a: Int } derives Ord + Key\n";
        let cats: Vec<&str> = bynk_ide::diagnose(text)
            .iter()
            .map(|d| d.error.category)
            .collect();
        for c in [
            "bynk.generics.redundant_eq_bound",
            "bynk.types.derives_automatic",
        ] {
            assert!(cats.contains(&c), "{c} in {cats:?}");
        }
        let out = fixed(text);
        assert!(out.contains("fn lt[T: Ord](a: T"), "{out}");
        assert!(out.contains("fn gt[T: Ord](a: T"), "{out}");
        assert!(out.contains("type Q = { a: Int } derives Ord\n"), "{out}");
        assert!(bynk_ide::diagnose(&out).is_empty(), "{out}");
    }

    /// Constrained generics slice 1: `not_orderable`'s "add `derives Ord`" is
    /// `MaybeIncorrect` (it changes what the type means), so fix-all never
    /// takes it — and is offered only as a non-preferred quick fix.
    #[test]
    fn never_applies_the_not_orderable_suggestion() {
        let text = "commons m\n\ntype R = { a: Int }\n\nfn lt(x: R, y: R) -> Bool { x < y }\n";
        let diagnostics = bynk_ide::diagnose(text);
        let d = diagnostics
            .iter()
            .find(|d| d.error.category == "bynk.types.not_orderable")
            .expect("`R` is not orderable");
        let s = d
            .error
            .suggestions
            .iter()
            .find(|s| s.message.contains("derives Ord"))
            .expect("a `derives Ord` suggestion");
        assert_eq!(
            s.applicability,
            bynk_syntax::error::Applicability::MaybeIncorrect
        );
        assert!(edits(text, &diagnostics).is_empty());
        assert_eq!(fixed(text), text);
        // …yet the quick fix is there, and not preferred.
        let uri = Url::parse("file:///a.bynk").unwrap();
        let fixes =
            crate::code_actions::quick_fixes(text, &diagnostics, d.error.span, &uri, Some(1));
        let [CodeActionOrCommand::CodeAction(a)] = fixes.as_slice() else {
            panic!("one quick fix: {fixes:?}");
        };
        assert!(a.title.contains("derives Ord"), "{}", a.title);
        assert_ne!(a.is_preferred, Some(true));
        assert_eq!(a.kind, Some(CodeActionKind::QUICKFIX));
    }

    /// The selection rule itself is tested in `bynk_syntax::fix`; this pins
    /// the adapter that feeds it the IDE's diagnostics.
    #[test]
    fn edits_adapts_ide_diagnostics_to_the_shared_rule() {
        let ds = [diag((4, 6), "b"), diag((0, 2), "a")];
        assert_eq!(
            edits("0123456789", &ds),
            vec![
                (Span::new(0, 2), "a".to_string()),
                (Span::new(4, 6), "b".to_string())
            ]
        );
        assert_eq!(apply("0123456789", &edits("0123456789", &ds)), "a23b6789");
    }
}
