//! #1885: hover on a unit name shows the unit's module doc, and the
//! documentation page merges a multi-file context.
//!
//! Each test analyses a real scratch project with `diagnose_project` and
//! drives the real hover ladder (`hover::hover_content`) or the real
//! documentation builder over its output, not a hand-built table. The project
//! puts `context shop`'s one module doc in `shop/pay.bynk`, so a cursor in
//! `shop/cart.bynk` can only find it by looking at a sibling file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::ProjectDiagnostics;
use bynk_lsp::hover;
use tower_lsp::lsp_types::Url;

const CART: &str = "\
// The cart half of the shop. A `--` comment, not a module doc.
context shop

uses lib.catalog

fn catalog() -> Int { 1 }

type CartId = String
";

const PAY: &str = "\
---
The shop: carts and payments.
---
context shop

type Amount = Int
";

const CATALOG: &str = "\
---
Product listings shared by every context.
---
commons lib.catalog

type Sku = String
";

const ORDERS: &str = "\
context orders

consumes shop

type OrderId = String
";

/// `cart.bynk` with an **empty** doc-block above its header: it documents
/// nothing, so it is not a module doc.
const CART_EMPTY_DOC: &str = "\
---
---
context shop

uses lib.catalog

fn catalog() -> Int { 1 }

type CartId = String
";

/// The scratch project, analysed. Returns the analysis and the root.
fn analysed(tag: &str) -> (ProjectDiagnostics, PathBuf) {
    analysed_with(tag, CART)
}

/// The scratch project with `cart` as `shop/cart.bynk`, on disk and analysed.
fn analysed_with(tag: &str, cart: &str) -> (ProjectDiagnostics, PathBuf) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("bynk-module-docs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, text) in [
        ("shop/cart.bynk", cart),
        ("shop/pay.bynk", PAY),
        ("lib/catalog.bynk", CATALOG),
        ("orders.bynk", ORDERS),
    ] {
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

/// The byte offset of `needle` at or after `anchor`.
fn at(text: &str, anchor: &str, needle: &str) -> usize {
    let a = text.find(anchor).expect("anchor");
    a + text[a..].find(needle).expect("needle")
}

/// Drive the real hover ladder for the cursor at `offset` in `name`. With
/// `with_files`, the ladder gets the project's other files the way
/// `Backend::hover` does (the calling file excluded); without, it must fall
/// back to the analysed round's snapshots.
fn hover_at(
    r: &ProjectDiagnostics,
    root: &Path,
    name: &str,
    offset: usize,
    with_files: bool,
) -> Option<String> {
    let (rel, text) = file(r, name);
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let others: HashMap<PathBuf, String> = r
        .files
        .iter()
        .filter(|f| f.source_path != rel)
        .map(|f| (root.join(&f.source_path), f.text.clone()))
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
        }),
        doc: Some((text, offset)),
        path: &uri.to_file_path().unwrap_or_default(),
        files: with_files.then_some(&others),
    })
}

/// Every error-severity diagnostic in the analysed project.
fn errors(r: &ProjectDiagnostics) -> Vec<String> {
    r.files
        .iter()
        .flat_map(|f| f.diagnostics.iter())
        .filter(|d| d.severity == bynk_syntax::error::Severity::Error)
        .map(|d| format!("{d:?}"))
        .collect()
}

#[test]
fn the_scratch_project_analyses_cleanly() {
    let (r, _) = analysed("clean");
    let diags = errors(&r);
    assert!(diags.is_empty(), "{diags:?}");
}

/// An empty `---`/`---` block above a header documents nothing, so it is not a
/// module doc (#1900 review). It does not count against the one-doc rule: an
/// empty block in `cart.bynk` beside the real doc in `pay.bynk` analyses
/// cleanly.
#[test]
fn an_empty_doc_block_is_not_a_module_doc_for_the_rule() {
    let (r, _) = analysed_with("empty-rule", CART_EMPTY_DOC);
    let diags = errors(&r);
    assert!(diags.is_empty(), "{diags:?}");
}

/// Nor does an empty block shadow a sibling's real doc on hover, though
/// `cart.bynk` sorts before `pay.bynk` and is searched first.
#[test]
fn an_empty_doc_block_does_not_shadow_a_siblings_doc_on_hover() {
    let (r, root) = analysed_with("empty-hover", CART_EMPTY_DOC);
    let (_, text) = file(&r, "orders.bynk");
    let offset = at(text, "consumes shop", "shop");
    let hover = hover_at(&r, &root, "orders.bynk", offset, true).expect("hover on consumes");
    assert!(hover.contains("The shop: carts and payments."), "{hover}");
}

/// Nor on the documentation page, where `cart.bynk`'s entries come first.
#[test]
fn an_empty_doc_block_does_not_shadow_a_siblings_doc_on_the_page() {
    let (r, root) = analysed_with("empty-page", CART_EMPTY_DOC);
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let (rel, _) = file(&r, "shop/cart.bynk");
    let wire = bynk_lsp::documentation_request::documentation_model_for(
        rel,
        &snapshots,
        &r.unit_sources,
        &root,
        None,
    )
    .expect("a page");
    assert_eq!(
        wire.unit_doc.as_deref(),
        Some("The shop: carts and payments.")
    );
}

/// The header name in `cart.bynk` shows the module doc that lives in its
/// sibling `pay.bynk`, through either source of sibling text.
#[test]
fn a_header_name_hovers_its_module_doc_from_a_sibling_file() {
    let (r, root) = analysed("header");
    let (_, text) = file(&r, "shop/cart.bynk");
    let offset = at(text, "context shop", "shop");
    for with_files in [true, false] {
        let hover = hover_at(&r, &root, "shop/cart.bynk", offset, with_files)
            .expect("hover on the header name");
        assert!(hover.starts_with("```bynk\ncontext shop\n```"), "{hover}");
        assert!(hover.contains("The shop: carts and payments."), "{hover}");
    }
}

/// A `uses` target hovers as the commons' module doc on every segment, and is
/// not answered by `fn catalog` — the same-named declaration the bare-name
/// rungs below it would find.
#[test]
fn a_uses_target_hovers_its_module_doc_not_a_same_named_declaration() {
    let (r, root) = analysed("uses");
    let (_, text) = file(&r, "shop/cart.bynk");
    for needle in ["lib", "catalog"] {
        let offset = at(text, "uses lib.catalog", needle);
        let hover = hover_at(&r, &root, "shop/cart.bynk", offset, true).expect("hover on uses");
        assert!(
            hover.starts_with("```bynk\ncommons lib.catalog\n```"),
            "{hover}"
        );
        assert!(
            hover.contains("Product listings shared by every context."),
            "{hover}"
        );
        assert!(!hover.contains("fn catalog"), "{hover}");
    }
}

/// A `consumes` target hovers as the consumed context's module doc, found in
/// whichever of its files carries it.
#[test]
fn a_consumes_target_hovers_its_module_doc() {
    let (r, root) = analysed("consumes");
    let (_, text) = file(&r, "orders.bynk");
    let offset = at(text, "consumes shop", "shop");
    let hover = hover_at(&r, &root, "orders.bynk", offset, true).expect("hover on consumes");
    assert!(hover.contains("The shop: carts and payments."), "{hover}");
}

/// A unit with no module doc still hovers as its kind and name.
#[test]
fn an_undocumented_unit_hovers_as_its_kind_and_name() {
    let (r, root) = analysed("undocumented");
    let (_, text) = file(&r, "orders.bynk");
    let offset = at(text, "context orders", "orders");
    let hover = hover_at(&r, &root, "orders.bynk", offset, true).expect("hover on orders");
    assert_eq!(hover, "```bynk\ncontext orders\n```\n");
}

/// The documentation page for either file of `context shop` is the whole
/// context: the one module doc, and every file's declarations, each lowered
/// against its own file and carrying that file's URI.
#[test]
fn the_documentation_page_merges_a_multi_file_context() {
    let (r, root) = analysed("page");
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    for name in ["shop/cart.bynk", "shop/pay.bynk"] {
        let (rel, _) = file(&r, name);
        let wire = bynk_lsp::documentation_request::documentation_model_for(
            rel,
            &snapshots,
            &r.unit_sources,
            &root,
            None,
        )
        .expect("a page");
        assert_eq!(wire.unit_name, "shop");
        assert_eq!(
            wire.unit_doc.as_deref(),
            Some("The shop: carts and payments.")
        );
        let names: Vec<&str> = wire.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["catalog", "CartId", "Amount"], "{name}");
        let amount = wire.entries.iter().find(|e| e.name == "Amount").unwrap();
        let cart_id = wire.entries.iter().find(|e| e.name == "CartId").unwrap();
        assert!(amount.uri.path().ends_with("shop/pay.bynk"), "{amount:?}");
        assert!(
            cart_id.uri.path().ends_with("shop/cart.bynk"),
            "{cart_id:?}"
        );
        // Each range is against the entry's own file: `type Amount` is on
        // line 6 (index 5) of pay.bynk, `type CartId` on line 8 of cart.bynk.
        assert_eq!(amount.range.start.line, 5);
        assert_eq!(cart_id.range.start.line, 7);
    }
}
