//! v0.28 (ADR 0057): the index's semantic-token payload — modifier flags on
//! user symbols (`refined` only with a refinement present; `opaque`
//! orthogonal) and the tokens-only `foreign_refs` side table routing
//! first-party references that `symbols` deliberately drops.

use std::path::{Path, PathBuf};

use bynk_check::index::{SymbolKind, SymbolModifiers};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic/src")
}

fn modifiers_of(result: &bynk_ide::ProjectDiagnostics, name: &str) -> SymbolModifiers {
    result
        .index
        .symbols
        .iter()
        .find(|(k, _)| k.unit == "shop.types" && k.kind == SymbolKind::Type && k.name == name)
        .unwrap_or_else(|| panic!("type `{name}` indexed"))
        .1
        .modifiers
}

#[test]
fn type_modifiers_follow_the_declaration() {
    let result = bynk_ide::diagnose_project(
        &fixture_root(),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root()).to_path_buf(),
        )),
    );

    let m = |refined, opaque| SymbolModifiers {
        refined,
        opaque,
        platform_native: false,
    };
    // `where` present → refined; `opaque` orthogonal; both compose.
    assert_eq!(modifiers_of(&result, "Age"), m(true, false));
    assert_eq!(modifiers_of(&result, "Token"), m(false, true));
    assert_eq!(modifiers_of(&result, "Code"), m(true, true));
    // The plain alias is `Refined { refinement: None }` — neither.
    assert_eq!(modifiers_of(&result, "Alias"), m(false, false));
}

#[test]
fn first_party_references_land_in_the_side_table() {
    let result = bynk_ide::diagnose_project(
        &fixture_root(),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root()).to_path_buf(),
        )),
    );

    // The `Kv` references in cache/store.bynk (`consumes` clause, `given`
    // clause, `Kv.get`/`Kv.put` call sites) are dropped from `symbols`
    // (synthetic def — the v0.25 rule) and routed to `foreign_refs`,
    // carrying `platformNative` from the declaring unit.
    let store = result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == "cache/store.bynk")
        .expect("context file analysed");
    let kv_refs: Vec<_> = result
        .index
        .foreign_refs
        .iter()
        .filter(|fr| fr.site.path == store.source_path)
        .collect();
    assert!(
        !kv_refs.is_empty(),
        "first-party `Kv` references recorded; foreign_refs: {:?}",
        result.index.foreign_refs
    );
    for fr in &kv_refs {
        assert_eq!(fr.kind, SymbolKind::Capability);
        assert!(fr.modifiers.platform_native, "Kv is platform-native");
        // Every recorded span is the name segment `Kv`.
        assert_eq!(&store.text[fr.site.span.range()], "Kv");
    }
    // Deduplicated and sorted by (path, span).
    let mut sorted = kv_refs.clone();
    sorted.sort_by(|a, b| a.site.cmp(&b.site));
    sorted.dedup_by(|a, b| a.site == b.site);
    assert_eq!(kv_refs.len(), sorted.len(), "deduped + sorted");

    // No first-party symbol leaked into `symbols` (the v0.25 invariant).
    assert!(
        result
            .index
            .symbols
            .keys()
            .all(|k| !k.unit.starts_with("bynk")),
        "synthetic units stay out of `symbols`"
    );
}
