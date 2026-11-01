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
