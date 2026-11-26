//! #4393: hover on a `Sender` / `Producer` refinement is a statement about what the Worker
//! refuses, so it must not be made for a refinement whose base is a local actor that shadows
//! the prelude one, wherever in the unit that actor is declared.
//!
//! The checker's actor table is the whole unit's. A context spans files, so the shadowing
//! `actor Sender` may be in a sibling of the file the refinement is in. These tests analyse a
//! real two-file scratch project and drive the real hover ladder and the real documentation
//! builder over it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::ProjectDiagnostics;
use bynk_lsp::hover;
use tower_lsp::lsp_types::Url;

/// The refinement, in `shop/b.bynk`.
const REFINING: &str = "\
context shop

actor Authenticated = Sender where Dmarc(DmarcPass)
";

/// A sibling that declares its own `Sender` actor: it shadows the prelude one for the unit.
const SHADOWING: &str = "\
context shop

actor Sender { auth = None }
";

/// A sibling of the same unit that declares nothing of the kind.
const PLAIN: &str = "\
context shop

type Id = String
";

/// A different unit that declares its own `Sender`, which is not this unit's.
const OTHER_UNIT: &str = "\
context orders

actor Sender { auth = None }
";

fn analysed(tag: &str, files: &[(&str, &str)]) -> (ProjectDiagnostics, PathBuf) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "bynk-actor-refinement-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, text) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    (r, root)
}

fn file<'r>(r: &'r ProjectDiagnostics, name: &str) -> (&'r Path, &'r str) {
    let f = r
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == name)
        .unwrap_or_else(|| panic!("{name} analysed"));
    (&f.source_path, &f.text)
}

/// Hover on the declared name `Authenticated` in `shop/b.bynk`, through the real ladder.
fn hover_on_the_refinement(r: &ProjectDiagnostics, root: &Path) -> String {
    let (rel, text) = file(r, "shop/b.bynk");
    let offset = text.find("Authenticated").expect("the refinement");
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let uri = Url::from_file_path(root.join(rel)).unwrap();
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
            analytics: &r.analytics,
        }),
        doc: Some((text, offset)),
        path: uri.to_file_path().ok().as_deref(),
        files: None,
    })
    .expect("hover on the refinement")
}

fn documented(r: &ProjectDiagnostics, root: &Path) -> String {
    let (primary, text) = file(r, "shop/b.bynk");
    let files: Vec<(PathBuf, String)> = r
        .files
        .iter()
        .map(|f| (root.join(&f.source_path), f.text.clone()))
        .collect();
    let primary_abs = root.join(primary);
    let model = bynk_ide::documentation::documentation_model_merged(
        (primary_abs.as_path(), text),
        files.iter().map(|(p, t)| (p.as_path(), t.as_str())),
    )
    .expect("the unit's page");
    model
        .entries
        .iter()
        .find(|e| e.name == "Authenticated")
        .expect("the actor's entry")
        .markdown
        .clone()
}

#[test]
fn a_sibling_that_shadows_sender_removes_the_note_on_hover() {
    let (r, root) = analysed(
        "hover-shadowed",
        &[("shop/a.bynk", SHADOWING), ("shop/b.bynk", REFINING)],
    );
    let hover = hover_on_the_refinement(&r, &root);
    assert!(hover.contains("actor Authenticated = Sender"), "{hover}");
    assert!(!hover.contains("Refines"), "{hover}");
}

#[test]
fn a_sibling_that_shadows_sender_removes_the_note_on_the_page() {
    let (r, root) = analysed(
        "page-shadowed",
        &[("shop/a.bynk", SHADOWING), ("shop/b.bynk", REFINING)],
    );
    let page = documented(&r, &root);
    assert!(page.contains("actor Authenticated = Sender"), "{page}");
    assert!(!page.contains("Refines"), "{page}");
}

/// Without a shadowing actor in the unit, the note is shown — a sibling that declares other
/// things does not take it away, and another unit's own `Sender` is not this unit's.
#[test]
fn an_unshadowed_refinement_keeps_its_note_beside_siblings_and_other_units() {
    for (tag, others) in [
        (
            "plain",
            [("shop/a.bynk", PLAIN), ("orders.bynk", OTHER_UNIT)],
        ),
        ("none", [("shop/a.bynk", PLAIN), ("other/c.bynk", PLAIN)]),
    ] {
        let mut files = vec![("shop/b.bynk", REFINING)];
        files.extend(others);
        let (r, root) = analysed(tag, &files);
        let hover = hover_on_the_refinement(&r, &root);
        assert!(hover.contains("Refines `Sender`"), "{tag}: {hover}");
        let page = documented(&r, &root);
        assert!(page.contains("Refines `Sender`"), "{tag}: {page}");
    }
}
