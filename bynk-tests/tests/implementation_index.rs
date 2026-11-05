//! v0.35 (ADR 0068): the implementation-nav graph — capability→provider
//! `ImplEdge`s assembled from each `provides Cap = Provider` clause (a
//! provides-flagged `Capability` reference whose owner is the provider). The
//! flag is what distinguishes the *provided* capability from the provider's own
//! `given` dependencies, which are also capability refs owned by the same
//! provider. The fixture (`160_provider_given_basic`) has exactly that shape:
//! `PoliteGreeter` provides `Greeter` *and* has `given Logger`.

use std::path::{Path, PathBuf};

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/160_provider_given_basic/src")
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

fn key(unit: &str, kind: SymbolKind, name: &str) -> SymbolKey {
    SymbolKey {
        unit: unit.to_string(),
        kind,
        name: name.to_string(),
    }
}

#[test]
fn impl_edges_link_capabilities_to_providers_excluding_given_deps() {
    let index = analyse(&fixture_root());

    let logger = key("demo", SymbolKind::Capability, "Logger");
    let greeter = key("demo", SymbolKind::Capability, "Greeter");
    let console = key("demo", SymbolKind::Provider, "ConsoleLogger");
    let polite = key("demo", SymbolKind::Provider, "PoliteGreeter");

    // `Logger` is implemented by `ConsoleLogger`. `PoliteGreeter` has
    // `given Logger`, but that's a dependency, not an implementation — so it is
    // NOT an impl edge for `Logger`.
    let logger_impls: Vec<&SymbolKey> = index.impls_of(&logger).map(|e| &e.provider).collect();
    assert_eq!(
        logger_impls,
        vec![&console],
        "Logger's only implementation is ConsoleLogger; PoliteGreeter's `given Logger` is excluded"
    );

    // The disambiguation is visible against the plain reference set: `Logger`
    // is referenced three times (the `provides Logger` clause, `given Logger`,
    // and the `Logger.info(...)` call) but has exactly one implementation edge.
    assert_eq!(
        index.symbols[&logger].refs.len(),
        3,
        "Logger: provides clause + `given Logger` + `Logger.info(...)`"
    );
    assert_eq!(index.impls_of(&logger).count(), 1);

    // `Greeter` is implemented by `PoliteGreeter`.
    let greeter_impls: Vec<&SymbolKey> = index.impls_of(&greeter).map(|e| &e.provider).collect();
    assert_eq!(greeter_impls, vec![&polite]);

    // Each impl site spells the capability name.
    for e in &index.impls {
        assert!(!e.site.span.range().is_empty());
    }
}

#[test]
fn unknown_or_non_capability_key_has_no_impls() {
    let index = analyse(&fixture_root());
    // A provider key is not a capability — no impls keyed by it.
    assert_eq!(
        index
            .impls_of(&key("demo", SymbolKind::Provider, "ConsoleLogger"))
            .count(),
        0
    );
    assert_eq!(
        index
            .impls_of(&key("demo", SymbolKind::Capability, "Nope"))
            .count(),
        0
    );
}
