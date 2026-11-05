//! v0.36 (ADR 0069): instance methods as first-class index symbols, keyed by
//! the compound `"Type.method"` name. The ref is recorded already-spelled from
//! the receiver type resolved in the checker, and resolves through the same
//! qualification as a cross-file type reference. The fixture has two types with
//! a same-named method (`Counter.bump` / `Gauge.bump`) so the compound key
//! proves shadowing-correct: the two never conflate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/216_method_index/src")
}

fn analyse(root: &Path) -> (ProjectIndex, HashMap<String, String>) {
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
    let snapshots = result
        .files
        .iter()
        .map(|f| {
            (
                f.source_path.to_string_lossy().replace('\\', "/"),
                f.text.clone(),
            )
        })
        .collect();
    (result.index, snapshots)
}

fn key(unit: &str, name: &str) -> SymbolKey {
    SymbolKey {
        unit: unit.to_string(),
        kind: SymbolKind::Method,
        name: name.to_string(),
    }
}

#[test]
fn methods_are_indexed_by_compound_name_with_shadowing_kept_distinct() {
    let (index, snapshots) = analyse(&fixture_root());

    let counter_bump = key("shop", "Counter.bump");
    let gauge_bump = key("shop", "Gauge.bump");

    // Both compound keys exist and are distinct — the same method name on two
    // types does not conflate.
    let cb = index
        .symbols
        .get(&counter_bump)
        .expect("Counter.bump indexed");
    let gb = index.symbols.get(&gauge_bump).expect("Gauge.bump indexed");

    // Each has its declaration plus exactly one call ref (Counter.bump from
    // `run`, Gauge.bump from `tick`) — never each other's.
    assert!(cb.def.is_some() && gb.def.is_some());
    assert_eq!(cb.refs.len(), 1, "Counter.bump called once (in `run`)");
    assert_eq!(gb.refs.len(), 1, "Gauge.bump called once (in `tick`)");

    // Every site (def + ref) spans only the `bump` member segment — never the
    // `Type.` prefix (this is what keeps rename editing the segment alone).
    for k in [&counter_bump, &gauge_bump] {
        let entry = &index.symbols[k];
        for site in entry.def.iter().chain(entry.refs.iter()) {
            let path = site.path.to_string_lossy().replace('\\', "/");
            let text = &snapshots[&path];
            assert_eq!(
                &text[site.span.range()],
                "bump",
                "{k:?} site must span the member segment only"
            );
        }
    }
}

#[test]
fn method_call_is_a_call_edge_with_its_free_fn_caller() {
    let (index, _) = analyse(&fixture_root());
    // `run` calls `Counter.bump` — a method callee now records a CallEdge.
    let run = SymbolKey {
        unit: "shop".to_string(),
        kind: SymbolKind::Fn,
        name: "run".to_string(),
    };
    let callees: Vec<&SymbolKey> = index.calls_from(&run).map(|e| &e.callee).collect();
    assert_eq!(callees, vec![&key("shop", "Counter.bump")]);
}
