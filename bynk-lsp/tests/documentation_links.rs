//! #2462 — intra-doc links on the documentation page, resolved server-side.
//!
//! Drives `documentation_model_for` against the analysed `examples/rate-limiter`
//! (the fixture `document_link_fixture.rs` uses): `[Limiter]` is documented on
//! the page, so it links to its entry; `[decide]` lives in another unit, so it
//! links to the source. Excluded from the published crate (`Cargo.toml`) —
//! reads `../examples`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_lsp::documentation_request::{
    DocLinkContext, WireDocEntry, WireDocLink, WireDocModel, documentation_model_for,
};

fn page() -> WireDocModel {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root")
        .join("examples")
        .join("rate-limiter")
        .join("src");
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let rel = r
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with("ratelimit.bynk"))
        .expect("ratelimit.bynk analysed")
        .source_path
        .clone();
    documentation_model_for(
        &rel,
        &snapshots,
        &r.unit_sources,
        &root,
        Some(DocLinkContext {
            index: &r.index,
            doc_scope: &r.doc_scope,
        }),
    )
    .expect("a page")
}

fn entry<'a>(wire: &'a WireDocModel, name: &str) -> (usize, &'a WireDocEntry) {
    wire.entries
        .iter()
        .enumerate()
        .find(|(_, e)| e.name == name)
        .unwrap_or_else(|| panic!("entry {name}"))
}

#[test]
fn a_link_to_a_declaration_on_the_page_targets_its_entry() {
    let wire = page();
    let (limiter, _) = entry(&wire, "Limiter");
    let (_, client_id) = entry(&wire, "ClientId");
    assert_eq!(client_id.links, vec![WireDocLink::Entry { entry: limiter }]);
    assert!(client_id.markdown.contains("[Limiter](#bynk-doc-link-0)"));
}

#[test]
fn a_link_to_another_unit_targets_its_source() {
    let wire = page();
    let (_, rate_view) = entry(&wire, "RateView");
    let source = rate_view
        .links
        .iter()
        .find_map(|l| match l {
            WireDocLink::Source { uri, range } => Some((uri, range)),
            _ => None,
        })
        .expect("`[decide]` links to its source");
    assert!(source.0.path().ends_with("/window.bynk"), "{}", source.0);
    assert!(source.1.start.line > 0 || source.1.start.character > 0);
    assert!(rate_view.markdown.contains("](#bynk-doc-link-"));
}

#[test]
fn without_a_resolver_the_markdown_is_untouched() {
    let rel = PathBuf::from("unit.bynk");
    let text = "context u\n\n---\nSee [Thing].\n---\ntype Thing = Int\n";
    let snapshots = HashMap::from([(rel.clone(), text.to_string())]);
    let wire = documentation_model_for(
        &rel,
        &snapshots,
        &HashMap::new(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        None,
    )
    .expect("a page");
    assert!(wire.entries[0].links.is_empty());
    assert!(wire.entries[0].markdown.contains("[Thing]"));
}

/// Analyse a one-file project written to a scratch directory (discovery reads
/// the disk; an overlay alone does not create a file) and return its page.
fn page_of(text: &str) -> WireDocModel {
    let root = std::env::temp_dir().join(format!("bynk-doc-links-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("a scratch directory");
    let rel = PathBuf::from("unit.bynk");
    std::fs::write(root.join(&rel), text).expect("the unit written");
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let wire = documentation_model_for(
        &rel,
        &snapshots,
        &r.unit_sources,
        &root,
        Some(DocLinkContext {
            index: &r.index,
            doc_scope: &r.doc_scope,
        }),
    );
    let _ = std::fs::remove_dir_all(&root);
    wire.unwrap_or_else(|| panic!("no page for the unit ({} files analysed)", r.files.len()))
}

#[test]
fn a_first_party_name_links_to_the_book() {
    let wire = page();
    let (_, api) = entry(&wire, "api");
    let href = api
        .links
        .iter()
        .find_map(|l| match l {
            WireDocLink::External { href } => Some(href),
            _ => None,
        })
        .expect("`[Clock.now]` links to the Book");
    assert!(
        href.starts_with("https://bynk-lang.org/book/reference/bynk-capabilities/#"),
        "{href}"
    );
}

#[test]
fn a_project_declaration_wins_over_a_first_party_name() {
    let wire = page_of(
        "context u\n\ncapability Clock {\n  fn now() -> Effect[Int]\n}\n\n---\nSee [Clock] and [Clock.now].\n---\ntype Thing = Int\n",
    );
    let (_, thing) = entry(&wire, "Thing");
    // The project's `Clock` shadows the first-party one: the link is never
    // sent to the Book.
    assert!(
        thing
            .links
            .iter()
            .all(|l| !matches!(l, WireDocLink::External { .. })),
        "{:?}",
        thing.links
    );
    // ...and the owner name did resolve, to the project's declaration.
    assert!(
        thing
            .links
            .iter()
            .any(|l| matches!(l, WireDocLink::Entry { .. } | WireDocLink::Source { .. })),
        "{:?}",
        thing.links
    );
}
