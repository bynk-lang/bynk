//! #2100 slice 6: `source.fixAll.bynk` end to end — a real project on disk
//! through `diagnose_project`, the checker-authored suggestions riding its
//! diagnostics, and the pure fix-all edit. Each positive case re-diagnoses
//! the fixed project (what the handler's DECISION B gate checks).

use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::fix_all;
use tower_lsp::lsp_types::*;

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-fix-all-{}-{}",
        test_name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    for (rel, contents) in files {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).expect("create parent");
        fs::write(&p, contents).expect("write file");
    }
    root.canonicalize().unwrap_or(root)
}

fn diagnose(root: &Path, rel: &str, text: Option<&str>) -> (String, Vec<bynk_ide::Diagnostic>) {
    let mut sources = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
        root.to_path_buf(),
    ));
    if let Some(text) = text {
        sources.insert(root.join(rel), text.to_string());
    }
    let result = bynk_ide::diagnose_project(root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new(rel))
        .expect("file analysed");
    (file.text.clone(), file.diagnostics.clone())
}

fn categories(diags: &[bynk_ide::Diagnostic]) -> Vec<&'static str> {
    diags.iter().map(|d| d.error.category).collect()
}

/// A context with three capabilities and two handlers, each with its own
/// `given` clause and body.
fn handlers(given_a: &str, body_a: &str, given_b: &str, body_b: &str) -> String {
    let mut src = String::from("context app.handlers\n\n");
    for (cap, n) in [("Alpha", 1), ("Beta", 2), ("Gamma", 3)] {
        src.push_str(&format!(
            "capability {cap} {{\n  fn ping() -> Effect[Int]\n}}\n\nprovides {cap} = {cap}Impl {{\n  fn ping() -> Effect[Int] {{\n    {n}\n  }}\n}}\n\n"
        ));
    }
    src.push_str(&format!(
        "service one {{\n  on call(x: Int) -> Effect[Int]{given_a} {{\n{body_a}\n  }}\n}}\n\nservice two {{\n  on call(x: Int) -> Effect[Int]{given_b} {{\n{body_b}\n  }}\n}}\n"
    ));
    src
}

const ALPHA: &str = "    let a <- Alpha.ping()\n    a";
const ALPHA_BETA: &str = "    let a <- Alpha.ping()\n    let b <- Beta.ping()\n    a + b";
const NONE: &str = "    42";

#[test]
fn fixes_every_auto_fixable_problem_in_one_edit() {
    // `one` lists `Gamma` it never uses; `two` uses `Beta` it never lists.
    let src = handlers(" given Alpha, Gamma", ALPHA, " given Alpha", ALPHA_BETA);
    let root = setup_project("every", &[("app/handlers.bynk", src.as_str())]);
    let (text, diags) = diagnose(&root, "app/handlers.bynk", None);
    assert_eq!(
        categories(&diags).len(),
        2,
        "two problems: {:?}",
        categories(&diags)
    );
    let edits = fix_all::edits(&diags);
    assert_eq!(edits.len(), 2);
    let fixed = fix_all::apply(&text, &edits);
    assert_eq!(
        fixed,
        handlers(" given Alpha", ALPHA, " given Alpha, Beta", ALPHA_BETA)
    );
    let (_, post) = diagnose(&root, "app/handlers.bynk", Some(&fixed));
    assert!(post.is_empty(), "fixed: {:?}", categories(&post));
}

#[test]
fn two_removals_from_one_clause_reach_clean() {
    // Both `Alpha` and `Beta` are unused in one clause: their removals meet
    // in the same list. Whatever fix-all takes in one pass must re-check
    // clean, and repeating it reaches the fully fixed text.
    let src = handlers(
        " given Alpha, Beta, Gamma",
        "    let g <- Gamma.ping()\n    g",
        "",
        NONE,
    );
    let root = setup_project("sameclause", &[("app/handlers.bynk", src.as_str())]);
    let (mut text, mut diags) = diagnose(&root, "app/handlers.bynk", None);
    for _ in 0..3 {
        let edits = fix_all::edits(&diags);
        if edits.is_empty() {
            break;
        }
        text = fix_all::apply(&text, &edits);
        diags = diagnose(&root, "app/handlers.bynk", Some(&text)).1;
        assert!(
            !categories(&diags)
                .iter()
                .any(|c| c.starts_with("bynk.parse")),
            "a pass broke the parse:\n{text}"
        );
    }
    assert_eq!(
        text,
        handlers(" given Gamma", "    let g <- Gamma.ping()\n    g", "", NONE)
    );
    assert!(diags.is_empty(), "residual: {:?}", categories(&diags));
}

#[test]
fn a_clean_file_has_nothing_to_fix() {
    let src = handlers(" given Alpha", ALPHA, "", NONE);
    let root = setup_project("clean", &[("app/handlers.bynk", src.as_str())]);
    let (_, diags) = diagnose(&root, "app/handlers.bynk", None);
    assert!(diags.is_empty(), "baseline: {:?}", categories(&diags));
    assert!(fix_all::edits(&diags).is_empty());
}

#[test]
fn the_action_is_a_versioned_fix_all_edit() {
    let src = handlers(" given Alpha, Gamma", ALPHA, "", NONE);
    let root = setup_project("action", &[("app/handlers.bynk", src.as_str())]);
    let (text, diags) = diagnose(&root, "app/handlers.bynk", None);
    let edits = fix_all::edits(&diags);
    let uri = Url::from_file_path(root.join("app/handlers.bynk")).unwrap();
    let CodeActionOrCommand::CodeAction(action) = fix_all::action(&text, &edits, &uri, Some(9))
    else {
        panic!("expected a CodeAction");
    };
    assert_eq!(action.kind, Some(CodeActionKind::new("source.fixAll.bynk")));
    let Some(DocumentChanges::Edits(doc_edits)) = &action.edit.unwrap().document_changes else {
        panic!("expected versioned document edits");
    };
    assert_eq!(doc_edits[0].text_document.version, Some(9));
    assert_eq!(doc_edits[0].edits.len(), 1);
}
