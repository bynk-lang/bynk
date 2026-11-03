//! #2834: a suite's helper `fn`s in the editor — hover, go-to-definition,
//! semantic tokens, completion and signature help. Read against the compiler's
//! own positive fixture, `2834_suite_fn_helpers`, analysed as a project.
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

const FIXTURE: &str = "../bynkc/tests/fixtures/positive/2834_suite_fn_helpers";

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

/// Hover on a call, and on the declaration, shows the helper's signature and
/// its doc block.
#[test]
fn a_helper_hovers_as_its_declaration() {
    let (r, rel, text, root) = analysed();
    let call = text.find("gh(\"push\")").unwrap();
    let at_call = hover_at(&r, &rel, &text, &root, call);
    assert!(
        at_call.contains("fn gh(event: String) -> RequestHeaders"),
        "{at_call}"
    );
    let decl = text.find("fn gh").unwrap() + 3;
    let at_decl = hover_at(&r, &rel, &text, &root, decl);
    assert!(
        at_decl.contains("fn gh(event: String) -> RequestHeaders"),
        "{at_decl}"
    );
    assert!(
        at_decl.contains("Request headers for a GitHub delivery"),
        "{at_decl}"
    );
}

/// The semantic-token producer reads the index: a helper is a `Fn` symbol with
/// a definition in its suite's file and a reference at each call, so the
/// declaration and its calls tokenise as a function.
#[test]
fn a_helper_is_a_function_symbol_in_the_index() {
    use bynk_check::index::SymbolKind;
    let (r, rel, text, _) = analysed();
    let (key, entry) = r
        .index
        .symbols
        .iter()
        .find(|(k, _)| k.kind == SymbolKind::Fn && k.name == "gh" && k.unit.contains("unit.bynk"))
        .expect("the unit suite's `gh` is indexed");
    let def = entry.def.as_ref().unwrap();
    assert!(def.path.ends_with(&rel) || rel.ends_with(&def.path));
    assert_eq!(&text[def.span.range()], "gh");
    // Two cases call it; the other suites' own `gh` are separate symbols.
    assert_eq!(entry.refs.len(), 2, "{key:?}: {:?}", entry.refs);
}

#[test]
fn a_helper_is_offered_in_a_case_body() {
    let (_, _, text, _) = analysed();
    let items = bynk_ide::completion::complete("\tlet r = ", &text, None);
    let gh = items
        .iter()
        .find(|i| i.label == "gh")
        .expect("`gh` offered");
    assert!(
        gh.detail.as_deref().unwrap_or("").contains("fn gh")
            || gh
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("gh(event: String)"),
        "{:?}",
        gh.detail
    );
}

#[test]
fn a_helper_call_gets_parameter_help() {
    let (_, _, text, _) = analysed();
    let label = bynk_ide::signature_help::resolve_label("gh", &text, None).expect("a label");
    assert_eq!(label, "gh(event: String) -> RequestHeaders");
}

/// The same at `system`, whose helpers record under the harness namespace.
#[test]
fn a_system_helper_hovers_as_its_declaration() {
    let (r, rel, text, root) = analysed_file("tests/system.bynk");
    let call = text.find("gh(\"push\")").unwrap();
    let at_call = hover_at(&r, &rel, &text, &root, call);
    assert!(
        at_call.contains("fn gh(event: String) -> RequestHeaders"),
        "{at_call}"
    );
}
