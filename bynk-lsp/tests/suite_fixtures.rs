//! #2841: a suite's `fixture`s in the editor — hover, the index (go-to-definition,
//! semantic tokens) and completion. Read against the compiler's
//! own positive fixture, `2841_suite_fixtures`, analysed as a project.
//!
//! Excluded from the published `bynk-lsp` crate (Cargo.toml), like the other
//! tests that read the compiler's fixtures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::ProjectDiagnostics;
use bynk_lsp::hover;
use tower_lsp::lsp_types::Url;

fn analysed() -> (ProjectDiagnostics, PathBuf, String, PathBuf) {
    analysed_file("tests/unit.bynk")
}

const FIXTURE: &str = "../bynk-tests/tests/fixtures/positive/2841_suite_fixtures";

fn analysed_file(file: &str) -> (ProjectDiagnostics, PathBuf, String, PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let f = r
        .files
        .iter()
        .find(|f| f.source_path.ends_with(file))
        .expect("the file is analysed");
    let (path, text) = (f.source_path.clone(), f.text.clone());
    let rel = r
        .index
        .symbols
        .values()
        .filter_map(|e| e.def.as_ref())
        .map(|d| d.path.clone())
        .find(|p| p.ends_with(file))
        .unwrap_or(path);
    (r, rel, text, root)
}

fn hover_at(r: &ProjectDiagnostics, rel: &Path, text: &str, root: &Path, offset: usize) -> String {
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let uri = Url::parse("file:///unit.bynk").unwrap();
    hover::hover_content(&hover::HoverInput {
        analysis: Some(hover::HoverAnalysis {
            index: &r.index,
            snapshots: &snapshots,
            locals: &r.locals,
            expr_types: &r.expr_types,
            tys: &r.ty_intern,
            rel,
            offset,
            capability_paths: &r.capability_paths,
            project_root: root,
            doc_scope: &r.doc_scope,
            boundary_info: &r.boundary_info,
            context_count: bynk_ide::wire_contract::real_context_count(
                &r.boundary_info,
                &r.unit_sources,
            ),
        }),
        doc: Some((text, offset)),
        path: uri.to_file_path().ok().as_deref(),
        files: None,
    })
    .expect("a hover")
}

/// Hover on a use, and on the declaration, shows the fixture and says it is
/// test-only.
#[test]
fn a_fixture_hovers_as_its_declaration() {
    let (r, rel, text, root) = analysed();
    let use_site = text.find("push.contains").unwrap();
    let at_use = hover_at(&r, &rel, &text, &root, use_site);
    assert!(
        at_use.contains("fixture push = \"fixtures/github/push.json\""),
        "{at_use}"
    );
    assert!(at_use.contains("never deployed"), "{at_use}");
    let decl = text.find("fixture push").unwrap() + "fixture ".len();
    let at_decl = hover_at(&r, &rel, &text, &root, decl);
    assert!(at_decl.contains("A captured delivery"), "{at_decl}");
}

/// The semantic-token producer reads the index: a fixture is a symbol with a
/// definition in its suite's file and a reference at each use.
#[test]
fn a_fixture_is_a_constant_symbol_in_the_index() {
    use bynk_check::index::SymbolKind;
    let (r, rel, text, _) = analysed();
    let (key, entry) = r
        .index
        .symbols
        .iter()
        .find(|(k, _)| {
            k.kind == SymbolKind::Embed && k.name == "push" && k.unit.contains("unit.bynk")
        })
        .expect("`push` is indexed");
    let def = entry.def.as_ref().unwrap();
    assert!(def.path.ends_with(&rel) || rel.ends_with(&def.path));
    assert_eq!(&text[def.span.range()], "push");
    assert!(!entry.refs.is_empty(), "{key:?}");
}

#[test]
fn fixtures_are_offered_in_a_case_body() {
    let (_, _, text, _) = analysed();
    let items = bynk_ide::completion::complete("\tlet r = ", &text, None);
    assert!(
        items.iter().any(|i| i.label == "push"),
        "{:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

#[test]
fn the_files_under_fixtures_complete_in_the_declaration() {
    let (_, _, text, root) = analysed();
    let items = bynk_ide::completion::complete_in(
        Some(&root),
        "fixture other = \"fixtures/gi",
        &text,
        None,
    );
    let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["fixtures/github/ping.json", "fixtures/github/push.json"]
    );
}
