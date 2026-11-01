//! #2680: editor surfaces over list patterns. The names a list pattern binds
//! (its elements and `..rest`) are locals like a `let`'s: hover shows their
//! type (`T` for an element, `List[T]` for the rest), semantic tokens colour
//! them as variables, and a `let [head, ..rest] = xs else { … }` gives them
//! inlay hints.

use bynk_lsp::hover;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Url;

const SRC: &str = "\
commons demo.lists

fn total(xs: List[Int]) -> Int {
  match xs {
    [] => 0
    [first, ..rest] => first + total(rest)
  }
}

fn second(xs: List[Int]) -> Int {
  let [_, next, ..tail] = xs else { 0 }
  next + tail.length()
}

fn narrowed(xs: List[Int]) -> Int {
  if xs is [lead, ..others] {
    lead + others.length()
  } else {
    0
  }
}
";

const PATH: &str = "demo/lists.bynk";

fn offset_of(needle: &str, n: usize) -> usize {
    SRC.match_indices(needle).nth(n).expect("needle present").0
}

fn analyse(test_name: &str) -> (bynk_ide::ProjectDiagnostics, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-list-patterns-{test_name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("demo")).expect("create test root");
    fs::write(root.join(PATH), SRC).expect("write file");
    let root = root.canonicalize().unwrap_or(root);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let errors: Vec<String> = result
        .files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .chain(&result.unattributed)
        .filter(|d| d.severity == bynk_syntax::Severity::Error)
        .map(|d| format!("{}: {}", d.error.category, d.error.message))
        .collect();
    assert!(
        errors.is_empty(),
        "the fixture should check cleanly: {errors:?}"
    );
    (result, root)
}

fn hover_at(r: &bynk_ide::ProjectDiagnostics, root: &Path, offset: usize) -> Option<String> {
    let rel = PathBuf::from(PATH);
    let snapshots: HashMap<PathBuf, String> = HashMap::from([(rel.clone(), SRC.to_string())]);
    let uri = Url::parse("file:///lists.bynk").unwrap();
    hover::hover_content(&hover::HoverInput {
        analysis: Some(hover::HoverAnalysis {
            index: &r.index,
            snapshots: &snapshots,
            locals: &r.locals,
            expr_types: &r.expr_types,
            tys: &r.ty_intern,
            rel: &rel,
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
        doc: Some((SRC, offset)),
        path: uri.to_file_path().ok().as_deref(),
        files: None,
    })
}

#[test]
fn element_and_rest_bindings_are_locals_with_their_types() {
    let (result, _) = analyse("locals");
    let locals = result
        .locals
        .get(&PathBuf::from(PATH))
        .expect("the file has locals");
    let ty = |name: &str| {
        locals
            .iter()
            .find(|l| l.name == name)
            .map(|l| l.ty.clone())
            .unwrap_or_else(|| panic!("`{name}` is not a local: {locals:?}"))
    };
    assert_eq!(ty("first"), "Int");
    assert_eq!(ty("rest"), "List[Int]");
    assert_eq!(ty("next"), "Int");
    assert_eq!(ty("tail"), "List[Int]");
    let local_tokens = bynk_ide::locals_nav::local_token_sites(locals, SRC);
    for name in ["first", "rest", "next", "tail"] {
        let at = offset_of(&format!("{name}"), 0);
        assert!(
            local_tokens.iter().any(|(s, decl)| s.start == at && *decl),
            "`{name}` should be a variable declaration token: {local_tokens:?}"
        );
    }
}

#[test]
fn bindings_hover_with_their_types() {
    let (result, root) = analyse("hover");
    for (needle, want) in [
        ("first, ..", "Int"),
        ("..rest]", "List[Int]"),
        ("next, ..", "Int"),
        ("..tail]", "List[Int]"),
    ] {
        let at = offset_of(needle, 0) + if needle.starts_with("..") { 3 } else { 1 };
        let text = hover_at(&result, &root, at).unwrap_or_else(|| panic!("hover on `{needle}`"));
        assert!(text.contains(want), "{needle}: {text}");
    }
}

#[test]
fn a_let_else_list_pattern_gets_inlay_hints() {
    let (result, _) = analyse("hints");
    let hints = result
        .hints
        .get(&PathBuf::from(PATH))
        .expect("the file has hints");
    let at = |needle: &str, skip: usize| {
        let start = offset_of(needle, 0) + skip;
        hints
            .iter()
            .find(|h| h.span.start == start)
            .map(|h| h.label.clone())
    };
    assert_eq!(at("next, ..", 0).as_deref(), Some(": Int"));
    assert_eq!(at("..tail]", 2).as_deref(), Some(": List[Int]"));
}
