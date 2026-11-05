//! v0.129 (#259): the refinement-family graph — `RefineEdge`s linking each
//! refined/opaque type (and plain alias) to the builtin base it is declared over.
//! The backing data for the "refinement families" codelens (every type over the
//! same base). Assembled from the type-def walk in `bynk-emit`'s index pass.
//!
//! The fixture (`134_url_shortener`) declares two `String`-refined types in one
//! commons — `ShortCode` and `LongUrl` — a family of two over `String`.

use std::path::{Path, PathBuf};

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/134_url_shortener/src")
}

fn analyse(root: &Path) -> ProjectIndex {
    let result = bynk_ide::diagnose_project(
        root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (root).to_path_buf(),
        )),
    );
    for f in &result.files {
        assert!(
            f.diagnostics.is_empty(),
            "fixture should be clean, got in {}: {:?}",
            f.source_path.display(),
            f.diagnostics
                .iter()
                .map(|d| d.error.category)
                .collect::<Vec<_>>()
        );
    }
    result.index
}

fn key(unit: &str, name: &str) -> SymbolKey {
    SymbolKey {
        unit: unit.to_string(),
        kind: SymbolKind::Type,
        name: name.to_string(),
    }
}

#[test]
fn refine_edges_group_refined_types_by_builtin_base() {
    let index = analyse(&fixture_root());

    let short_code = key("shortener.analytics", "ShortCode");
    let long_url = key("shortener.analytics", "LongUrl");

    // Both refined types record `String` as their base.
    let base = index
        .refined_base(&short_code)
        .expect("ShortCode is a refined type over a builtin");
    assert_eq!(base.name(), "String");
    assert_eq!(
        index.refined_base(&long_url).map(|b| b.name()),
        Some("String"),
    );

    // The `String` family is exactly {ShortCode, LongUrl}.
    let family: Vec<&SymbolKey> = index.refinements_over(base).map(|e| &e.ty).collect();
    assert_eq!(family.len(), 2, "two String-refined types: {family:?}");
    assert!(family.contains(&&short_code) && family.contains(&&long_url));
}

#[test]
fn a_non_refinement_or_unknown_key_has_no_base() {
    let index = analyse(&fixture_root());
    // A capability is not a refined type — no base, and it is not in any family.
    let cap = SymbolKey {
        unit: "shortener.analytics".to_string(),
        kind: SymbolKind::Capability,
        name: "ShortCode".to_string(),
    };
    assert_eq!(index.refined_base(&cap), None);
    assert_eq!(
        index.refined_base(&key("shortener.analytics", "Nope")),
        None
    );
    // Every recorded edge resolves to a real, surviving index symbol.
    for e in &index.refinements {
        assert!(
            index.symbols.contains_key(&e.ty),
            "a refine edge must point at a live index symbol: {:?}",
            e.ty
        );
    }
}
