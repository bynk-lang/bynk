//! #2098: `bynk/explain` — the in-editor, offline explanation of a diagnostic
//! code, and the "Explain …" code action that opens it.
//!
//! The explanation data is compiled into the toolchain
//! ([`bynk_syntax::diagnostics::explain`]), so the server already holds the
//! exact text for the code it just emitted: serving it from here is
//! version-matched by construction and needs no network and no `bynk` on
//! `PATH`. The content is the same curated blurb, example and Book link that
//! `bynk explain <code>` prints (`bynk/src/explain.rs`); this module only
//! re-shapes it as Markdown for a client to render.
//!
//! **Transport (DECISION A).** A custom request, `bynk/explain { code } →
//! { title, markdown, href } | null`, advertised under
//! `ServerCapabilities.experimental.explain` like the other custom requests.
//! The request is **not** document-scoped: the params carry only the code, so
//! the client can also look up a code it saw in CI output. `null` for any code
//! with no curated explanation — a known-but-uncurated code and an unknown one
//! alike; telling those apart (and saying so gracefully) is the client's job.
//!
//! **Code action (DECISION B).** For every diagnostic in the requested range
//! whose code is curated, a `quickfix` that carries a client-side command
//! ([`EXPLAIN_COMMAND`]) rather than an edit. It is appended after every real
//! fix and never marked preferred, so it cannot preempt one. Because the
//! command is executed by the *client*, the action is only offered to a client
//! that says it implements it — `experimental.explainDiagnosticCommand: true`
//! in its `ClientCapabilities` (see [`client_supports_explain_command`]). A
//! client that does not would otherwise show a lightbulb entry that does
//! nothing when chosen.

use bynk_syntax::diagnostics;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, Command};

/// The client-side command the "Explain …" code action invokes, with the
/// diagnostic code as its single argument. The VS Code extension registers it
/// (`vscode-bynk/src/explainView.ts`).
pub const EXPLAIN_COMMAND: &str = "bynk.explainDiagnostic";

/// The `ClientCapabilities.experimental` key a client sets to `true` to say it
/// implements [`EXPLAIN_COMMAND`], and so wants the "Explain …" code action.
pub const CLIENT_CAPABILITY: &str = "explainDiagnosticCommand";

/// The `bynk/explain` request payload: just the diagnostic code.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExplainParams {
    pub code: String,
}

/// The `bynk/explain` response for a curated code.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WireExplanation {
    /// The diagnostic code, the panel's heading.
    pub title: String,
    /// The blurb, then the example as a fenced `bynk` block. Carries no link
    /// and no raw HTML; the client renders the "Read more" link from `href`.
    pub markdown: String,
    /// The hosted-Book page for this code, the same URL as the diagnostic's
    /// `codeDescription` and `bynk explain`'s "Learn more" line.
    pub href: String,
}

/// The explanation for `code`, or `None` when the compiler curates none (an
/// uncurated real code, or not a code at all).
pub fn explanation_for(code: &str) -> Option<WireExplanation> {
    let e = diagnostics::explain(code)?;
    let markdown = format!(
        "{blurb}\n\n## Example\n\n```bynk\n{example}\n```\n",
        blurb = e.blurb,
        example = e.example.trim_end(),
    );
    Some(WireExplanation {
        title: e.code.to_string(),
        markdown,
        href: e.href(),
    })
}

/// Whether the client advertised `experimental.explainDiagnosticCommand: true`.
pub fn client_supports_explain_command(experimental: Option<&serde_json::Value>) -> bool {
    experimental
        .and_then(|v| v.get(CLIENT_CAPABILITY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// One "Explain `code`" action per distinct curated code among the diagnostics
/// whose span intersects `requested` (the same keying rule as
/// [`crate::code_actions::quick_fixes`]). In diagnostic order.
pub fn explain_actions(
    diagnostics: &[bynk_ide::Diagnostic],
    requested: Span,
) -> Vec<CodeActionOrCommand> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for d in diagnostics {
        let code = d.error.category;
        if !intersects(d.error.span, requested) || seen.contains(&code) {
            continue;
        }
        if diagnostics::explain(code).is_none() {
            continue;
        }
        seen.push(code);
        let title = format!("Explain `{code}`");
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: title.clone(),
            kind: Some(CodeActionKind::QUICKFIX),
            command: Some(Command {
                title,
                command: EXPLAIN_COMMAND.to_string(),
                arguments: Some(vec![serde_json::Value::String(code.to_string())]),
            }),
            is_preferred: Some(false),
            ..Default::default()
        }));
    }
    out
}

/// Closed intersection over half-open spans, as in `code_actions`: a cursor
/// on either boundary of the diagnostic still matches.
fn intersects(a: Span, b: Span) -> bool {
    a.start <= b.end && b.start <= a.end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curated_code_has_title_markdown_and_href() {
        let e = diagnostics::explain("bynk.resolve.unknown_type").unwrap();
        let w = explanation_for("bynk.resolve.unknown_type").expect("curated");
        assert_eq!(w.title, "bynk.resolve.unknown_type");
        assert!(w.markdown.starts_with(e.blurb), "{}", w.markdown);
        assert!(w.markdown.contains("```bynk\n"), "{}", w.markdown);
        for line in e.example.lines() {
            assert!(w.markdown.contains(line), "example line {line:?} missing");
        }
        assert_eq!(w.href, "https://bynk-lang.org/book/reference/types/");
    }

    #[test]
    fn every_curated_code_explains() {
        for e in diagnostics::EXPLANATIONS {
            let w = explanation_for(e.code).expect("curated");
            assert_eq!(w.href, e.href());
            assert!(!w.markdown.contains('<'), "no raw HTML: {}", e.code);
        }
    }

    #[test]
    fn an_uncurated_or_unknown_code_is_none() {
        assert!(diagnostics::lookup("bynk.resolve.duplicate_type").is_some());
        assert!(explanation_for("bynk.resolve.duplicate_type").is_none());
        assert!(explanation_for("bynk.not.a_real_code").is_none());
        assert!(explanation_for("").is_none());
    }

    #[test]
    fn the_client_capability_gate() {
        assert!(!client_supports_explain_command(None));
        assert!(!client_supports_explain_command(Some(&serde_json::json!(
            {}
        ))));
        assert!(!client_supports_explain_command(Some(
            &serde_json::json!({ "explainDiagnosticCommand": "yes" })
        )));
        assert!(client_supports_explain_command(Some(
            &serde_json::json!({ "explainDiagnosticCommand": true })
        )));
    }

    #[test]
    fn params_deserialize_from_the_wire_shape() {
        let p: ExplainParams =
            serde_json::from_value(serde_json::json!({ "code": "bynk.resolve.unknown_name" }))
                .unwrap();
        assert_eq!(p.code, "bynk.resolve.unknown_name");
    }
}
