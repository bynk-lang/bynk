//! v0.36 (ADR 0069, slice 2): record fields and capability operations as
//! first-class index symbols, keyed by the compound `"Type.field"` / `"Cap.op"`
//! name. Fields are referenced from every form — read access, construction
//! labels, and spread overrides — so rename is complete; ops from their calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/217_field_op_index/src")
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

fn key(name: &str, kind: SymbolKind) -> SymbolKey {
    SymbolKey {
        unit: "shop".to_string(),
        kind,
        name: name.to_string(),
    }
}

#[test]
fn fields_are_indexed_with_every_reference_form() {
    let (index, snapshots) = analyse(&fixture_root());

    let cents = key("Money.cents", SymbolKind::Field);
    let currency = key("Money.currency", SymbolKind::Field);

    let cents_e = index.symbols.get(&cents).expect("Money.cents indexed");
    let currency_e = index
        .symbols
        .get(&currency)
        .expect("Money.currency indexed");

    // `cents`: read access in `total`, construction label + read access in
    // `relabel` = 3 references (rename would touch all of them).
    assert!(cents_e.def.is_some());
    assert_eq!(
        cents_e.refs.len(),
        3,
        "cents: total access + relabel label + relabel access"
    );
    // `currency`: only the construction label in `relabel`.
    assert_eq!(currency_e.refs.len(), 1, "currency: relabel label");

    // Every site spans the bare field segment (never the `Money.` prefix).
    for k in [&cents, &currency] {
        let entry = &index.symbols[k];
        let seg = k.name.rsplit('.').next().unwrap();
        for site in entry.def.iter().chain(entry.refs.iter()) {
            let path = site.path.to_string_lossy().replace('\\', "/");
            assert_eq!(&snapshots[&path][site.span.range()], seg);
        }
    }
}

#[test]
fn capability_ops_are_indexed_from_their_calls() {
    let (index, snapshots) = analyse(&fixture_root());

    let now = key("Clock.now", SymbolKind::CapabilityOp);
    let entry = index.symbols.get(&now).expect("Clock.now indexed");

    // def in the capability, one call in the service handler.
    assert!(entry.def.is_some());
    assert_eq!(entry.refs.len(), 1, "Clock.now called once (in `api`)");
    for site in entry.def.iter().chain(entry.refs.iter()) {
        let path = site.path.to_string_lossy().replace('\\', "/");
        assert_eq!(&snapshots[&path][site.span.range()], "now");
    }
}
