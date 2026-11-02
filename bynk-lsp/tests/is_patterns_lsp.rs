//! #2701: the names an `is` pattern binds are locals, scoped to the branch the
//! test guards (then-branch, else-branch of a negation, right operand of `&&`):
//! hover, go-to-definition, references and rename resolve them there, and not
//! outside.

use bynk_lsp::hover;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Url;

const SRC: &str = "\
commons demo.isbind

type Point = { x: Int, y: Int }

fn opt(o: Option[Int]) -> Int {
  if o is Some(v) {
    v + 1
  } else {
    0
  }
}

fn chain(a: Option[Int], b: Option[Int]) -> Bool {
  a is Some(p) && b is Some(q) && p < q
}

fn negated(o: Option[Int]) -> Int {
  if !(o is Some(n)) {
    0
  } else {
    n * 2
  }
}

fn list(xs: List[Int]) -> Int {
  if xs is [lead, ..others] {
    lead + others.length()
  } else {
    0
  }
}

fn rec(pt: Point) -> Int {
  if pt is (Point { x, y: why, .. }) {
    x + why
  } else {
    0
  }
}
";

const PATH: &str = "demo/isbind.bynk";

fn offset_of(needle: &str, n: usize) -> usize {
    SRC.match_indices(needle).nth(n).expect("needle present").0
}

fn analyse(test_name: &str) -> (bynk_ide::ProjectDiagnostics, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-is-patterns-{test_name}-{}",
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
    let uri = Url::parse("file:///isbind.bynk").unwrap();
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

fn locals(r: &bynk_ide::ProjectDiagnostics) -> &Vec<bynk_check::locals::LocalBinding> {
    r.locals
        .get(&PathBuf::from(PATH))
        .expect("the file has locals")
}

#[test]
fn is_bindings_are_locals_with_their_types() {
    let (result, _) = analyse("types");
    let locals = locals(&result);
    let ty = |name: &str| {
        locals
            .iter()
            .find(|l| l.name == name)
            .map(|l| l.ty.clone())
            .unwrap_or_else(|| panic!("`{name}` is not a local: {locals:?}"))
    };
    assert_eq!(ty("v"), "Int");
    assert_eq!(ty("p"), "Int");
    assert_eq!(ty("q"), "Int");
    assert_eq!(ty("n"), "Int");
    assert_eq!(ty("lead"), "Int");
    assert_eq!(ty("others"), "List[Int]");
    assert_eq!(ty("x"), "Int");
    assert_eq!(ty("why"), "Int");
    // Recorded once each, not once per checker visit.
    for name in ["v", "p", "q", "n", "lead", "others", "x", "why"] {
        assert_eq!(
            locals.iter().filter(|l| l.name == name).count(),
            1,
            "`{name}` should be recorded once: {locals:?}"
        );
    }
}

#[test]
fn is_bindings_hover_at_definition_and_use() {
    let (result, root) = analyse("hover");
    for (needle, skip, want) in [
        ("Some(v)", 5, "let v: Int"),
        ("v + 1", 0, "let v: Int"),
        ("..others]", 2, "others: List[Int]"),
        ("others.length", 0, "others: List[Int]"),
        ("n * 2", 0, "let n: Int"),
        ("p < q", 4, "let q: Int"),
        ("p < q", 0, "let p: Int"),
        ("y: why", 3, "why: Int"),
    ] {
        let at = offset_of(needle, 0) + skip;
        let text = hover_at(&result, &root, at).unwrap_or_else(|| panic!("hover on `{needle}`"));
        assert!(text.contains(want), "{needle}: {text}");
    }
}

#[test]
fn goto_definition_and_references_resolve_is_bindings() {
    let (result, _) = analyse("nav");
    let locals = locals(&result);
    let def = offset_of("Some(v)", 0) + 5;
    let use_at = offset_of("v + 1", 0);
    assert_eq!(
        bynk_ide::locals_nav::local_definition_at(locals, SRC, use_at).map(|s| s.start),
        Some(def)
    );
    let sites = bynk_ide::locals_nav::local_sites_at(locals, SRC, def).expect("sites");
    assert_eq!(
        sites.iter().map(|s| s.start).collect::<Vec<_>>(),
        vec![def, use_at],
        "rename/references cover the binding and its use"
    );
    // `&&` chain: `p`'s use is in the right operand of the second `&&`.
    let p_def = offset_of("Some(p)", 0) + 5;
    let p_sites = bynk_ide::locals_nav::local_sites_at(locals, SRC, p_def).expect("sites");
    assert_eq!(p_sites.len(), 2, "{p_sites:?}");
    // The negation's else-branch sees `n`.
    let n_def = offset_of("Some(n)", 0) + 5;
    let n_sites = bynk_ide::locals_nav::local_sites_at(locals, SRC, n_def).expect("sites");
    assert_eq!(n_sites.len(), 2, "{n_sites:?}");
}

#[test]
fn is_bindings_are_not_in_scope_outside_their_branch() {
    let (result, _) = analyse("scope");
    let locals = locals(&result);
    // `v` is the then-branch's only: not in scope in the else branch.
    let else_at = offset_of("0\n  }\n}\n\nfn chain", 0);
    let visible: Vec<&str> = bynk_check::locals::locals_at(locals, else_at)
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    assert!(!visible.contains(&"v"), "{visible:?}");
    assert!(visible.contains(&"o"), "{visible:?}");
    let then_at = offset_of("v + 1", 0);
    let visible: Vec<&str> = bynk_check::locals::locals_at(locals, then_at)
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    assert!(visible.contains(&"v"), "{visible:?}");
}
