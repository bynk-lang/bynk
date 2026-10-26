//! #847: `bynk/documentationModel` — the documentation page and its wire
//! conversion in `bynk_lsp::documentation_request`, driven directly
//! (not through a JSON-RPC round trip — `Backend`'s committed-round gating is
//! exercised by `committed_analysis_serves_the_stale_round_without_refreshing`
//! in-crate; this file covers what lives in `documentation_request`: building
//! the model from a file's text and lowering its spans to LSP ranges). Same
//! posture as `sequence_request.rs`.

const DOC_CTX: &str = r#"---
The account context — sign-up and the running balance.
---
context bank.account

consumes bynk { Clock }

---
An amount in the smallest currency unit; never negative.
---
type Money = Int where NonNegative

capability Ledger {
  ---
  Records a posting and returns the new balance.
  ---
  fn post(delta: Int) -> Effect[Int]
}

---
The public HTTP surface for an account.
---
service Accounts from http {
  ---
  The current balance for the addressed account.
  ---
  on GET("/balance") () -> Effect[Int] given Ledger {
    Ledger.post(0)
  }
}

agent Balance {
  key holder: String
  store cents: Cell[Int]
  ---
  Applies a signed delta and returns the running total.
  ---
  on call apply(delta: Int) -> Effect[Int] {
    let _ <- cents.update((c) => c + delta)
    cents
  }
}
"#;

/// The wire page for a one-file project holding `text`, through the server's
/// real builder (`documentation_model_for`). With no other file of the unit in
/// the round, the merged page is that file's page.
fn page(text: &str) -> Option<bynk_lsp::documentation_request::WireDocModel> {
    use std::collections::HashMap;
    use std::path::PathBuf;
    let rel = PathBuf::from("unit.bynk");
    let snapshots = HashMap::from([(rel.clone(), text.to_string())]);
    bynk_lsp::documentation_request::documentation_model_for(
        &rel,
        &snapshots,
        &HashMap::new(),
        &std::env::temp_dir(),
        None,
    )
}

/// A fully-documented context: every declaration appears, in `document_symbols`
/// order and hierarchy, with its doc reaching the rendered Markdown — and the
/// wire form lowers every span to a range without dropping an entry.
#[test]
fn documentation_model_aggregates_and_lowers_the_whole_file() {
    let wire = page(DOC_CTX).expect("a context has a documentation page");

    let names: Vec<(&str, u32)> = wire
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.depth))
        .collect();
    assert_eq!(
        names,
        vec![
            ("Money", 0),
            ("Ledger", 0),
            ("Ledger.post", 1),
            ("Accounts", 0),
            ("on GET(\"/balance\")", 1),
            ("Balance", 0),
            ("Balance.apply", 1),
        ]
    );

    // The unit metadata, and each entry's span lowered to a range in this
    // file (every entry names it).
    assert_eq!(wire.unit_name, "bank.account");
    assert_eq!(wire.unit_kind, "context");
    assert_eq!(
        wire.unit_doc.as_deref(),
        Some("The account context — sign-up and the running balance.")
    );
    assert!(
        wire.entries
            .iter()
            .all(|e| e.uri.path().ends_with("/unit.bynk"))
    );

    // A handler entry's range is a real, non-empty span inside the file — the
    // click-to-code target the webview reveals.
    let apply = wire
        .entries
        .iter()
        .find(|e| e.name == "Balance.apply")
        .expect("the agent handler entry");
    assert!(apply.documented);
    assert!(apply.markdown.contains("Applies a signed delta"));
    // A real, non-empty click-to-code target (the `apply` name span).
    let (s, e) = (apply.range.start, apply.range.end);
    assert!((s.line, s.character) < (e.line, e.character));
}

/// A file with undocumented declarations still lists them, flagged for the
/// coverage placeholder (Decision B) — the model is the same whether or not a
/// declaration carries docs.
#[test]
fn undocumented_declarations_survive_into_the_wire_model() {
    let src = "commons demo.x {\n\
               type Undocumented = Int\n\
               fn helper(n: Int) -> Int { n }\n\
               }";
    let wire = page(src).expect("a model");
    assert_eq!(wire.unit_kind, "commons");
    assert!(wire.entries.iter().all(|e| !e.documented));
    // Signatures are still present — a reference, not only a comment dump.
    assert!(wire.entries.iter().all(|e| e.markdown.contains("```bynk")));
}

/// The params must deserialize from the **camelCase** wire shape the client
/// actually sends (`textDocument`), not the Rust field name (`text_document`).
/// This is the edge no `documentation_model_for` test covers — the direct query
/// never round-trips JSON-RPC params — and the exact mismatch that shipped
/// latent in #846's sequence request (fixed alongside this).
#[test]
fn params_deserialize_from_the_camelcase_wire_shape() {
    let wire = r#"{"textDocument":{"uri":"file:///x.bynk"}}"#;
    let params: bynk_lsp::documentation_request::DocumentationModelParams =
        serde_json::from_str(wire).expect("camelCase textDocument must deserialize");
    assert_eq!(params.text_document.uri.as_str(), "file:///x.bynk");
}

/// A `suite` unit has no documentation page (Tier 1) — the request answers
/// empty, exactly as the server's non-project / no-committed-round paths do.
#[test]
fn suite_units_yield_no_model() {
    let src = "suite for demo.app {\n\
               case works {\n\
               expect true\n\
               }\n\
               }";
    assert!(page(src).is_none());
}
