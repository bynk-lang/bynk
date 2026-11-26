//! #4183: contexts and commons are index symbols, so hover, go to definition,
//! find references and semantic tokens work on a unit name at each place one is
//! written: the unit's own header, a `consumes`, a `uses`, and `IsContext(…)`.
//! Rename of a unit is the next step and is refused until then.
//!
//! Each test analyses a real scratch project with `diagnose_project` and drives
//! the real hover ladder and index queries over its output.
//!
//! The project: `ops.admin` and `ops.billing` (the latter restricts a service to
//! `ops.admin` with `IsContext`), the commons `lib.catalog`, and the context
//! `shop`, split across `shop/cart.bynk` and `shop/pay.bynk`, which `uses`
//! `lib.catalog` and `consumes` `ops.billing`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::ProjectDiagnostics;
use bynk_lsp::{hover, index_queries};
use tower_lsp::lsp_types::Url;

const ADMIN: &str = "\
---
Operations: the context that may issue refunds.
---
context ops.admin

service audit {
  on call(id: String) -> Effect[Int] by Caller {
    Effect.pure(1)
  }
}
";

const BILLING: &str = "\
context ops.billing

actor Admin = Caller where IsContext(ops.admin)

service refunds {
  on call(id: String) -> Effect[Int] by Admin {
    Effect.pure(1)
  }
}
";

const CATALOG: &str = "\
commons lib.catalog

type Sku = String
";

const CART: &str = "\
context shop

uses lib.catalog
consumes ops.billing

type CartId = String
";

const PAY: &str = "\
context shop

type Amount = Int
";

/// A scratch project of `files`, written to disk and analysed.
fn project(tag: &str, files: &[(&str, &str)]) -> (ProjectDiagnostics, PathBuf) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("bynk-unit-symbols-{tag}-{}", std::process::id()));
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

const MANIFEST: &str = "[package]\nname = \"acme/shop\"\n";

/// The project of the module docs, analysed, and required to analyse cleanly.
fn analysed(tag: &str) -> (ProjectDiagnostics, PathBuf) {
    let (r, root) = project(
        tag,
        &[
            ("bynk.toml", MANIFEST),
            ("ops/admin.bynk", ADMIN),
            ("ops/billing.bynk", BILLING),
            ("lib/catalog.bynk", CATALOG),
            ("shop/cart.bynk", CART),
            ("shop/pay.bynk", PAY),
        ],
    );
    let errors: Vec<String> = r
        .files
        .iter()
        .flat_map(|f| f.diagnostics.iter())
        .filter(|d| d.severity == bynk_syntax::error::Severity::Error)
        .map(|d| format!("{d:?}"))
        .collect();
    assert!(
        errors.is_empty(),
        "the project analyses cleanly: {errors:?}"
    );
    (r, root)
}

/// The file `name` (project-relative, `/`-separated) of the analysis.
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

/// A site's path, `/`-separated, and the text its span covers.
fn site_text(
    r: &ProjectDiagnostics,
    path: &Path,
    span: bynk_syntax::span::Span,
) -> (String, String) {
    let name = path.to_string_lossy().replace('\\', "/");
    let (_, text) = file(r, &name);
    (name, text[span.range()].to_string())
}

fn hover_at(r: &ProjectDiagnostics, root: &Path, name: &str, offset: usize) -> Option<String> {
    let (rel, text) = file(r, name);
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
        }),
        doc: Some((text, offset)),
        path: uri.to_file_path().ok().as_deref(),
        files: None,
    })
}

/// The (project-relative path, covered text) of every site of the unit at
/// `offset` in `name`: the definition first, then references in order.
fn sites_at(r: &ProjectDiagnostics, name: &str, offset: usize) -> Vec<(String, String)> {
    let (rel, _) = file(r, name);
    index_queries::sites_for(&r.index, rel, offset, true)
        .expect("a symbol at the cursor")
        .into_iter()
        .map(|s| site_text(r, &s.path, s.span))
        .collect()
}

#[test]
fn a_unit_is_defined_at_its_header() {
    let (r, _) = analysed("definition");
    let (_, text) = file(&r, "ops/billing.bynk");
    // From `IsContext(ops.admin)`, from `consumes ops.billing`, from `uses
    // lib.catalog`: each lands on the name in the `context …` / `commons …` line.
    let cases = [
        (
            "ops/billing.bynk",
            at(text, "IsContext(", "ops.admin"),
            ("ops/admin.bynk", "ops.admin"),
        ),
        (
            "shop/cart.bynk",
            at(CART, "consumes ops.billing", "ops.billing"),
            ("ops/billing.bynk", "ops.billing"),
        ),
        (
            "shop/cart.bynk",
            at(CART, "uses lib.catalog", "lib.catalog"),
            ("lib/catalog.bynk", "lib.catalog"),
        ),
    ];
    for (from, offset, (want_file, want_text)) in cases {
        let (rel, _) = file(&r, from);
        let (_, def) = index_queries::definition_at(&r.index, rel, offset).expect("a definition");
        let (got_file, got_text) = site_text(&r, &def.path, def.span);
        assert_eq!(
            (got_file.as_str(), got_text.as_str()),
            (want_file, want_text),
            "from {from} at {offset}"
        );
    }
}

#[test]
fn every_segment_of_a_dotted_name_resolves() {
    let (r, _) = analysed("segments");
    let (rel, _) = file(&r, "shop/cart.bynk");
    let start = at(CART, "consumes ops.billing", "ops.billing");
    // The first segment, the dot, and the last character all hit the one name.
    for offset in [start, start + 3, start + "ops.billing".len() - 1] {
        let (key, _) = index_queries::definition_at(&r.index, rel, offset)
            .unwrap_or_else(|| panic!("no definition at +{}", offset - start));
        assert_eq!(key.name, "ops.billing");
    }
}

#[test]
fn find_references_reaches_consumes_uses_and_is_context() {
    let (r, _) = analysed("references");

    // `ops.admin`: its header, and the `IsContext` that names it.
    let (_, billing) = file(&r, "ops/billing.bynk");
    let admin = sites_at(
        &r,
        "ops/billing.bynk",
        at(billing, "IsContext(", "ops.admin"),
    );
    assert_eq!(
        admin,
        [
            ("ops/admin.bynk".to_string(), "ops.admin".to_string()),
            ("ops/billing.bynk".to_string(), "ops.admin".to_string()),
        ]
    );

    // `ops.billing`: its header, and `shop`'s `consumes`.
    let billing_sites = sites_at(
        &r,
        "shop/cart.bynk",
        at(CART, "consumes ops.billing", "ops.billing"),
    );
    assert_eq!(
        billing_sites,
        [
            ("ops/billing.bynk".to_string(), "ops.billing".to_string()),
            ("shop/cart.bynk".to_string(), "ops.billing".to_string()),
        ]
    );

    // `lib.catalog`, a commons: the same, through `uses`.
    let catalog = sites_at(
        &r,
        "shop/cart.bynk",
        at(CART, "uses lib.catalog", "lib.catalog"),
    );
    assert_eq!(
        catalog,
        [
            ("lib/catalog.bynk".to_string(), "lib.catalog".to_string()),
            ("shop/cart.bynk".to_string(), "lib.catalog".to_string()),
        ]
    );
}

#[test]
fn a_unit_split_across_files_has_one_definition_and_a_reference_per_other_header() {
    let (r, _) = analysed("split");
    let sites = sites_at(&r, "shop/pay.bynk", at(PAY, "context shop", "shop"));
    assert_eq!(
        sites,
        [
            ("shop/cart.bynk".to_string(), "shop".to_string()),
            ("shop/pay.bynk".to_string(), "shop".to_string()),
        ],
        "the first file's header defines; the other is a reference"
    );
    // From either header, the one definition.
    for from in ["shop/cart.bynk", "shop/pay.bynk"] {
        let (rel, text) = file(&r, from);
        let offset = at(text, "context shop", "shop");
        let (_, def) = index_queries::definition_at(&r.index, rel, offset).expect("a definition");
        assert_eq!(
            def.path.to_string_lossy().replace('\\', "/"),
            "shop/cart.bynk"
        );
    }
}

#[test]
fn hover_shows_the_kind_the_path_the_file_and_the_module_doc() {
    let (r, root) = analysed("hover");

    // On `IsContext(ops.admin)`: the unit, where it is declared, and its doc.
    let (_, billing) = file(&r, "ops/billing.bynk");
    let hover = hover_at(
        &r,
        &root,
        "ops/billing.bynk",
        at(billing, "IsContext(", "ops.admin"),
    )
    .expect("hover on the IsContext name");
    assert!(
        hover.starts_with("```bynk\ncontext ops.admin\n```\n"),
        "{hover}"
    );
    assert!(hover.contains("Declared in `ops/admin.bynk`."), "{hover}");
    assert!(
        hover.contains("Operations: the context that may issue refunds."),
        "{hover}"
    );

    // On a `consumes` of a context, and a `uses` of a commons: each as its kind.
    let consumes = hover_at(
        &r,
        &root,
        "shop/cart.bynk",
        at(CART, "consumes ops.billing", "billing"),
    )
    .expect("hover on a consumes");
    assert!(
        consumes.starts_with("```bynk\ncontext ops.billing\n```\n"),
        "{consumes}"
    );
    assert!(
        consumes.contains("Declared in `ops/billing.bynk`."),
        "{consumes}"
    );
    let uses = hover_at(
        &r,
        &root,
        "shop/cart.bynk",
        at(CART, "uses lib.catalog", "lib"),
    )
    .expect("hover on a uses");
    assert!(
        uses.starts_with("```bynk\ncommons lib.catalog\n```\n"),
        "{uses}"
    );
    assert!(uses.contains("Declared in `lib/catalog.bynk`."), "{uses}");
}

#[test]
fn a_split_units_hover_finds_the_module_doc_in_a_sibling_file() {
    // The doc lives in `shop/pay.bynk`; the hover is on `shop/cart.bynk`'s
    // header, which is the definition, and the doc must come from the sibling.
    let cart = "context shop\n\ntype CartId = String\n";
    let (r, root) = project(
        "split-doc",
        &[
            ("bynk.toml", MANIFEST),
            ("shop/cart.bynk", cart),
            (
                "shop/pay.bynk",
                "---\nThe shop: carts and payments.\n---\ncontext shop\n\ntype Amount = Int\n",
            ),
        ],
    );
    let hover = hover_at(
        &r,
        &root,
        "shop/cart.bynk",
        at(cart, "context shop", "shop"),
    )
    .expect("hover on the header");
    assert!(hover.contains("The shop: carts and payments."), "{hover}");
    assert!(hover.contains("Declared in `shop/cart.bynk`."), "{hover}");
}

/// The legend index of the `namespace` token type.
fn namespace_type() -> u32 {
    index_queries::semantic_tokens_legend()
        .token_types
        .iter()
        .position(|t| t.as_str() == "namespace")
        .expect("`namespace` is in the legend") as u32
}

/// Every token of `name` as (line, column, length, type, modifiers), decoded
/// from the LSP delta encoding.
fn tokens_of(r: &ProjectDiagnostics, name: &str) -> Vec<(u32, u32, u32, u32, u32)> {
    let (rel, text) = file(r, name);
    let tokens = index_queries::semantic_tokens(&r.index, &[], &[], rel, text, None);
    let (mut line, mut col) = (0u32, 0u32);
    let mut out = Vec::new();
    for t in &tokens {
        if t.delta_line > 0 {
            line += t.delta_line;
            col = t.delta_start;
        } else {
            col += t.delta_start;
        }
        out.push((line, col, t.length, t.token_type, t.token_modifiers_bitset));
    }
    out
}

/// The 0-based (line, column) of `needle` at or after `anchor` in `text`.
fn position(text: &str, anchor: &str, needle: &str) -> (u32, u32) {
    let off = at(text, anchor, needle);
    let line = text[..off].matches('\n').count() as u32;
    let col = (off - text[..off].rfind('\n').map_or(0, |n| n + 1)) as u32;
    (line, col)
}

#[test]
fn a_unit_name_is_a_namespace_token_per_segment() {
    let (r, _) = analysed("tokens");
    let ns = namespace_type();

    // `consumes ops.billing`: `ops` and `billing`, nothing over the dot.
    let tokens = tokens_of(&r, "shop/cart.bynk");
    let (line, col) = position(CART, "consumes ops.billing", "ops.billing");
    let on_name: Vec<_> = tokens
        .iter()
        .filter(|(l, c, ..)| *l == line && *c >= col && *c < col + "ops.billing".len() as u32)
        .collect();
    assert_eq!(
        on_name,
        [&(line, col, 3, ns, 0), &(line, col + 4, 7, ns, 0),],
        "one namespace token per segment, none for the `.`"
    );

    // `uses lib.catalog` likewise.
    let (line, col) = position(CART, "uses lib.catalog", "lib.catalog");
    assert!(tokens.contains(&(line, col, 3, ns, 0)), "{tokens:?}");
    assert!(tokens.contains(&(line, col + 4, 7, ns, 0)), "{tokens:?}");
}

#[test]
fn the_header_name_carries_the_declaration_modifier() {
    let (r, _) = analysed("declaration");
    let ns = namespace_type();
    let tokens = tokens_of(&r, "ops/admin.bynk");
    let (line, col) = position(ADMIN, "context ops.admin", "ops.admin");
    // The `declaration` modifier is bit 0.
    assert!(tokens.contains(&(line, col, 3, ns, 1)), "{tokens:?}");
    assert!(tokens.contains(&(line, col + 4, 5, ns, 1)), "{tokens:?}");

    // And the `IsContext(…)` name is a reference, without it.
    let tokens = tokens_of(&r, "ops/billing.bynk");
    let (line, col) = position(BILLING, "IsContext(", "ops.admin");
    assert!(tokens.contains(&(line, col, 3, ns, 0)), "{tokens:?}");
    assert!(tokens.contains(&(line, col + 4, 5, ns, 0)), "{tokens:?}");
}

#[test]
fn a_context_that_does_not_exist_resolves_to_nothing() {
    // A misspelt `IsContext(…)` name is a diagnostic, not a reference: it has no
    // definition, and it is not a site of the context it was meant to name.
    let billing = BILLING.replace("ops.admin", "ops.admn");
    let (r, _) = project(
        "unknown",
        &[
            ("bynk.toml", MANIFEST),
            ("ops/admin.bynk", ADMIN),
            ("ops/billing.bynk", &billing),
        ],
    );
    let (rel, text) = file(&r, "ops/billing.bynk");
    assert_eq!(text, billing, "the analysed text is the typo'd one");
    let offset = at(text, "IsContext(", "ops.admn");
    assert!(index_queries::definition_at(&r.index, rel, offset).is_none());
    let admin = sites_at(
        &r,
        "ops/admin.bynk",
        at(ADMIN, "context ops.admin", "ops.admin"),
    );
    assert_eq!(
        admin,
        [("ops/admin.bynk".to_string(), "ops.admin".to_string())],
        "only the definition: the misspelling is not a reference to it"
    );
}

#[test]
fn a_unit_is_renameable_as_one_whole_dotted_name() {
    let (r, _) = analysed("rename");
    let (rel, _) = file(&r, "shop/cart.bynk");
    let offset = at(CART, "consumes ops.billing", "ops.billing");
    // `prepareRename` answers, and the range is the whole dotted name.
    let (key, site) =
        index_queries::prepare_rename(&r.index, rel, offset).expect("prepareRename answers");
    assert_eq!(key.name, "ops.billing");
    assert_eq!(&CART[site.span.range()], "ops.billing");
    // The plan edits the definition and every reference, whole.
    let plan =
        index_queries::plan_rename(&r.index, rel, offset, "ops.invoicing").expect("a unit renames");
    let edited: Vec<(String, String)> = plan
        .edits
        .iter()
        .flat_map(|(path, spans)| {
            spans
                .iter()
                .map(|span| site_text(&r, path, *span))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        edited,
        [
            ("ops/billing.bynk".to_string(), "ops.billing".to_string()),
            ("shop/cart.bynk".to_string(), "ops.billing".to_string()),
        ]
    );
}

#[test]
fn units_are_workspace_symbols_and_lens_targets() {
    let (r, _) = analysed("workspace");
    let found: Vec<&str> = index_queries::workspace_symbols(&r.index, "ops.")
        .into_iter()
        .map(|(k, _)| k.name.as_str())
        .collect();
    assert!(
        found.contains(&"ops.admin") && found.contains(&"ops.billing"),
        "{found:?}"
    );
    // The reference-count lens at `ops.admin`'s header counts the `IsContext`.
    let (rel, _) = file(&r, "ops/admin.bynk");
    let lenses = index_queries::code_lenses(&r.index, rel);
    let (_, refs) = lenses
        .iter()
        .find(|(def, _)| ADMIN[def.span.range()] == *"ops.admin")
        .expect("a lens on the unit's header");
    assert_eq!(refs.len(), 1);
}

#[test]
fn a_unit_is_not_a_call_hierarchy_item() {
    // A unit is indexed, but nothing calls it: `prepareCallHierarchy` answers
    // nothing on a unit name, as it did before units were symbols.
    let (r, _) = analysed("call-hierarchy");
    let (rel, _) = file(&r, "shop/cart.bynk");
    let offset = at(CART, "consumes ops.billing", "ops.billing");
    assert!(index_queries::definition_at(&r.index, rel, offset).is_some());
    assert!(index_queries::prepare_call_hierarchy(&r.index, rel, offset).is_none());
}

const TOKENS_ADAPTER: &str = "\
adapter tokens {
  binding \"./tokens.binding.ts\" requires { \"jose\": \"^5\" }

  exports capability { Jwt }

  capability Jwt {
    fn sign(sub: String, secret: String) -> Effect[String]
  }

  provides Jwt = JoseJwt
}
";

const TOKENS_BINDING: &str = "\
import type { Jwt } from \"./tokens.js\";

export class JoseJwt implements Jwt {
  async sign(sub: string, secret: string): Promise<string> {
    return `${sub}.${secret}`;
  }
}
";

#[test]
fn an_adapter_is_not_a_unit_symbol_and_still_hovers_as_before() {
    // The issue's decision A covers contexts and commons. An adapter is neither
    // defined nor referenced as a unit, so a `consumes` of one is resolved by
    // the lexical hover rung exactly as it was.
    let app = "context app\n\nconsumes tokens\n\ntype Id = String\n";
    let (r, root) = project(
        "adapter",
        &[
            ("bynk.toml", MANIFEST),
            ("src/tokens.bynk", TOKENS_ADAPTER),
            ("src/tokens.binding.ts", TOKENS_BINDING),
            ("src/app.bynk", app),
        ],
    );
    assert!(
        r.index
            .symbols
            .keys()
            .any(|k| k.kind == bynk_check::index::SymbolKind::Unit && k.name == "app"),
        "the context `app` is indexed, so the index was built"
    );
    assert!(
        r.index
            .symbols
            .keys()
            .all(|k| k.kind != bynk_check::index::SymbolKind::Unit || k.name != "tokens"),
        "the adapter `tokens` is not a unit symbol"
    );
    let (rel, text) = file(&r, "src/app.bynk");
    let offset = at(text, "consumes tokens", "tokens");
    assert!(index_queries::definition_at(&r.index, rel, offset).is_none());
    let hover = hover_at(&r, &root, "src/app.bynk", offset).expect("hover on the adapter");
    assert!(hover.starts_with("```bynk\nadapter tokens\n```"), "{hover}");
    assert!(!hover.contains("Declared in"), "{hover}");
}
