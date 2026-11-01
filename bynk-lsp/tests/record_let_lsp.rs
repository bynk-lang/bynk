//! #2654: editor surfaces over a destructuring `let` — `let Item { sku, qty: n, .. } = item`.
//! The record pattern is slice 1's node and the `let` reaches the checker as the
//! `let … else` one-arm `match` (#2010), so these tests pin that the names the
//! pattern binds behave like `let` bindings (hint, local, go-to-definition,
//! semantic token), that its field names are coloured as properties, and that
//! the hidden binder surfaces nowhere.

use bynk_lsp::{hover, index_queries};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Url;

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-record-let-{test_name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create test root");
    for (rel, contents) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&p, contents).expect("write file");
    }
    root.canonicalize().unwrap_or(root)
}

const SRC: &str = "\
commons demo.items

type Item = { sku: String, qty: Int, note: String }

fn label(item: Item) -> String {
  let Item { sku, qty: n, .. } = item
  \"\\(sku) x\\(n)\"
}
";

const PATH: &str = "demo/items.bynk";

fn offset_of(needle: &str, n: usize) -> usize {
    SRC.match_indices(needle).nth(n).expect("needle present").0
}

fn analyse(test_name: &str) -> bynk_ide::ProjectDiagnostics {
    analyse_in(test_name).0
}

fn analyse_in(test_name: &str) -> (bynk_ide::ProjectDiagnostics, PathBuf) {
    let root = setup_project(test_name, &[(PATH, SRC)]);
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
    let uri = Url::parse("file:///items.bynk").unwrap();
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
fn bound_names_get_hints_and_the_hidden_binder_none() {
    let result = analyse("hints");
    let hints = result
        .hints
        .get(&PathBuf::from(PATH))
        .expect("the file has hints");
    let at = |needle: &str| {
        let start = offset_of(needle, 0);
        hints
            .iter()
            .find(|h| h.span.start == start)
            .map(|h| h.label.clone())
    };
    assert_eq!(at("sku, ").as_deref(), Some(": String"));
    assert_eq!(at("n, ").as_deref(), Some(": Int"));
    assert!(
        hints.iter().all(|h| h.span.start < h.span.end),
        "no hint may sit on the hidden binder's empty span: {hints:?}"
    );
}

#[test]
fn bound_names_are_locals_for_the_rest_of_the_block() {
    let result = analyse("locals");
    let locals = result
        .locals
        .get(&PathBuf::from(PATH))
        .expect("the file has locals");
    let names: Vec<&str> = locals.iter().map(|l| l.name.as_str()).collect();
    assert!(names.contains(&"sku") && names.contains(&"n"), "{names:?}");
    assert!(
        locals.iter().all(|l| !l.name.starts_with("__")),
        "the hidden binder must not be a local: {names:?}"
    );
    let sku = locals.iter().find(|l| l.name == "sku").unwrap();
    assert_eq!(sku.ty, "String");
    let use_at = offset_of("(sku)", 0) + 1;
    assert!(sku.scope.start <= use_at && use_at < sku.scope.end);
    let def = bynk_ide::locals_nav::local_definition_at(locals, SRC, use_at)
        .expect("the use of `sku` resolves");
    assert_eq!(def.start, offset_of("sku, ", 0));
}

#[test]
fn field_names_are_properties_and_bound_names_variables() {
    let result = analyse("tokens");
    let path = PathBuf::from(PATH);
    let locals = result.locals.get(&path).expect("the file has locals");
    let local_tokens = bynk_ide::locals_nav::local_token_sites(locals, SRC);
    // The shorthand `sku` is the binding's declaration; its use is plain.
    for (at, declaration) in [
        (offset_of("sku, ", 0), true),
        (offset_of("(sku)", 0) + 1, false),
    ] {
        assert!(
            local_tokens
                .iter()
                .any(|(s, decl)| s.start == at && *decl == declaration),
            "`sku` at {at} should be a variable token \
             (declaration: {declaration}): {local_tokens:?}"
        );
    }
    let tokens =
        index_queries::semantic_tokens(&result.index, &local_tokens, &[], &path, SRC, None);
    assert!(
        tokens.iter().all(|t| t.length > 0),
        "no empty tokens: {tokens:?}"
    );
    // `qty` in `qty: n` is a field reference, coloured as a property.
    let legend = index_queries::semantic_tokens_legend();
    let property = legend
        .token_types
        .iter()
        .position(|t| t.as_str() == "property")
        .expect("`property` is in the legend") as u32;
    let off = offset_of("qty: n", 0);
    let want_line = SRC[..off].matches('\n').count() as u32;
    let want_col = (off - SRC[..off].rfind('\n').map_or(0, |n| n + 1)) as u32;
    let (mut line, mut col) = (0u32, 0u32);
    let mut found = Vec::new();
    for t in &tokens {
        if t.delta_line > 0 {
            line += t.delta_line;
            col = t.delta_start;
        } else {
            col += t.delta_start;
        }
        if (line, col) == (want_line, want_col) {
            found.push((t.length, t.token_type));
        }
    }
    assert_eq!(found, vec![(3, property)]);
}

#[test]
fn field_and_bound_names_hover() {
    let (result, root) = analyse_in("hover");
    // A listed field hovers as the record's field.
    let field = hover_at(&result, &root, offset_of("qty: n", 0) + 1)
        .expect("hover on the `qty` field label");
    assert!(
        field.contains("qty: Int") && field.contains("A field of `Item`"),
        "{field}"
    );
    // A bound name hovers with its type, at its binding and at a use.
    for at in [offset_of("sku, ", 0) + 1, offset_of("(sku)", 0) + 2] {
        let local = hover_at(&result, &root, at).expect("hover on `sku`");
        assert!(local.contains("sku") && local.contains("String"), "{local}");
        assert!(!local.contains("__letElse"), "{local}");
    }
    let renamed = hover_at(&result, &root, offset_of("n, ", 0)).expect("hover on `n`");
    assert!(renamed.contains("Int"), "{renamed}");
}
