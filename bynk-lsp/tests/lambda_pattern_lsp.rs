//! #2712: editor surfaces over a record-pattern lambda parameter —
//! `items.map((Item { sku, qty: n, .. }) => …)`. The parser desugars the
//! parameter to a hidden `__lambdaArg<offset>` binder and slice 2's `else`-less
//! `let` (whose own hidden binder is `__letElse<offset>`), so these tests pin
//! that the names the pattern binds behave like `let` bindings (hint, local,
//! semantic token, hover) and that neither hidden binder surfaces in any
//! diagnostic, hint, local, hover, semantic token or signature label.

use bynk_lsp::{hover, index_queries};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Url;

const SRC: &str = "\
commons demo.items

type Item = { sku: String, qty: Int, note: String }

fn apply[A, B](xs: List[A], f: (A) -> B) -> List[B] {
  xs.map(f)
}

fn labels(items: List[Item]) -> List[String] {
  items.map((Item { sku, qty: n, note: _, .. }) => \"\\(sku) x\\(n)\")
}

fn total(items: List[Item]) -> Int {
  items.fold(0, (sum, Item { qty, .. }) => sum + qty)
}

fn viaApply(items: List[Item]) -> List[Int] {
  apply(items, (Item { qty, .. }) => qty)
}
";

const PATH: &str = "demo/items.bynk";

fn offset_of(needle: &str, n: usize) -> usize {
    SRC.match_indices(needle).nth(n).expect("needle present").0
}

fn analyse_in(test_name: &str) -> (bynk_ide::ProjectDiagnostics, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-lambda-pattern-{test_name}-{}",
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

fn is_hidden(text: &str) -> bool {
    text.contains("__lambdaArg") || text.contains("__letElse")
}

#[test]
fn bound_names_get_hints_and_the_hidden_binders_none() {
    let (result, _) = analyse_in("hints");
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
    assert_eq!(at("sku, qty").as_deref(), Some(": String"));
    assert_eq!(at("n, note").as_deref(), Some(": Int"));
    assert!(
        hints.iter().all(|h| h.span.start < h.span.end),
        "no hint may sit on a hidden binder's empty span: {hints:?}"
    );
    assert!(
        hints.iter().all(|h| !is_hidden(&h.label)),
        "no hint may name a hidden binder: {hints:?}"
    );
}

#[test]
fn bound_names_are_locals_and_the_hidden_binders_are_not() {
    let (result, _) = analyse_in("locals");
    let locals = result
        .locals
        .get(&PathBuf::from(PATH))
        .expect("the file has locals");
    let names: Vec<&str> = locals.iter().map(|l| l.name.as_str()).collect();
    for want in ["sku", "n", "sum", "qty"] {
        assert!(names.contains(&want), "`{want}` missing: {names:?}");
    }
    assert!(
        locals.iter().all(|l| !l.name.starts_with("__")),
        "a hidden binder must not be a local: {names:?}"
    );
    let sku = locals.iter().find(|l| l.name == "sku").unwrap();
    assert_eq!(sku.ty, "String");
    let use_at = offset_of("(sku)", 0) + 1;
    assert!(sku.scope.start <= use_at && use_at < sku.scope.end);
    let def = bynk_ide::locals_nav::local_definition_at(locals, SRC, use_at)
        .expect("the use of `sku` resolves");
    assert_eq!(def.start, offset_of("sku, qty", 0));
}

#[test]
fn field_names_are_properties_and_no_token_is_empty() {
    let (result, _) = analyse_in("tokens");
    let path = PathBuf::from(PATH);
    let locals = result.locals.get(&path).expect("the file has locals");
    let local_tokens = bynk_ide::locals_nav::local_token_sites(locals, SRC);
    assert!(
        local_tokens
            .iter()
            .any(|(s, decl)| s.start == offset_of("sku, qty", 0) && *decl),
        "`sku` should be a variable declaration token: {local_tokens:?}"
    );
    let tokens =
        index_queries::semantic_tokens(&result.index, &local_tokens, &[], &path, SRC, None);
    assert!(
        tokens.iter().all(|t| t.length > 0),
        "no empty tokens: {tokens:?}"
    );
    let legend = index_queries::semantic_tokens_legend();
    let property = legend
        .token_types
        .iter()
        .position(|t| t.as_str() == "property")
        .expect("`property` is in the legend") as u32;
    // `qty` in `qty: n` is a field reference, coloured as a property.
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
fn field_and_bound_names_hover_and_no_offset_hovers_a_hidden_binder() {
    let (result, root) = analyse_in("hover");
    let field = hover_at(&result, &root, offset_of("qty: n", 0) + 1)
        .expect("hover on the `qty` field label");
    assert!(
        field.contains("qty: Int") && field.contains("A field of `Item`"),
        "{field}"
    );
    for at in [offset_of("sku, qty", 0) + 1, offset_of("(sku)", 0) + 2] {
        let local = hover_at(&result, &root, at).expect("hover on `sku`");
        assert!(local.contains("sku") && local.contains("String"), "{local}");
    }
    // Every offset of every lambda line: whatever hovers, it names no hidden binder.
    for needle in ["items.map((", "items.fold(", "apply(items"] {
        let start = offset_of(needle, 0);
        let end = start + SRC[start..].find('\n').unwrap();
        for at in start..end {
            if let Some(h) = hover_at(&result, &root, at) {
                assert!(!is_hidden(&h), "hover at {at} names a hidden binder: {h}");
            }
        }
    }
}

#[test]
fn a_call_taking_a_pattern_lambda_labels_its_own_signature() {
    let (result, root) = analyse_in("signature");
    let label = bynk_ide::signature_help::resolve_label("apply", SRC, None)
        .expect("the call's callee resolves");
    assert!(label.contains("f: A -> B"), "{label}");
    assert!(!is_hidden(&label), "{label}");
    // Hovering the call shows the callee's signature, not the lambda's binder.
    let h = hover_at(&result, &root, offset_of("apply(items", 0) + 1).expect("hover on `apply`");
    assert!(!is_hidden(&h), "{h}");
    // Completion inside a pattern parameter offers the record's unlisted fields.
    let got = bynk_ide::completion::complete("  items.map((Item { sku, ", SRC, None);
    let labels: Vec<&str> = got.iter().map(|c| c.label.as_str()).collect();
    assert!(
        labels.contains(&"qty") && labels.contains(&"..") && !labels.contains(&"sku"),
        "{labels:?}"
    );
}

#[test]
fn no_diagnostic_names_a_hidden_binder() {
    let bad = "\
commons demo.items

type Item = { sku: String, qty: Int }
type Other = { sku: String }

fn a(xs: List[Other]) -> List[Int] {
  xs.map((Item { qty, .. }) => qty)
}

fn b(xs: List[Item]) -> List[Int] {
  xs.map((Item { nope, .. }) => 1)
}

fn c(xs: List[Item]) -> List[Int] {
  xs.map((Item { qty, qty, .. }) => qty)
}

fn d() -> Int {
  let f = (Item { qty, .. }) => qty
  f(1)
}
";
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-lambda-pattern-diag-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("demo")).expect("create test root");
    fs::write(root.join(PATH), bad).expect("write file");
    let root = root.canonicalize().unwrap_or(root);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let diagnostics: Vec<_> = result
        .files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .chain(&result.unattributed)
        .collect();
    assert!(diagnostics.len() >= 4, "{} diagnostics", diagnostics.len());
    for d in diagnostics {
        let e = &d.error;
        let mut text = vec![e.message.clone()];
        text.extend(e.notes.iter().cloned());
        text.extend(e.labels.iter().map(|(_, l)| l.clone()));
        for t in text {
            assert!(!is_hidden(&t), "{}: {t}", e.category);
        }
        assert!(
            e.span.start < e.span.end,
            "{}: empty primary span",
            e.category
        );
    }
}
