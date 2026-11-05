//! v0.30.2 (ADR 0063): the per-file expression-type capture that backs
//! `.`-member completion's receiver typing. ADR 0094 (slice 4) lifts the clean-file
//! ceiling: in Analyse mode the checker's best-effort partial types are recorded
//! even when the file has an error elsewhere, so completion works mid-edit.

use bynk_check::checker::{Ty, TyId};
use bynk_check::expr_types::type_at_offset;
use bynk_syntax::ast::BaseType;
use bynk_syntax::span::Span;
use std::path::{Path, PathBuf};

fn fixture_root(which: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/inlay")
        .join(which)
        .join("src")
}

/// A fixture file's `(expr span, Ty)` entries alongside its analysed text.
fn types_for(
    result: &bynk_ide::ProjectDiagnostics,
    file: &str,
) -> Option<(Vec<(Span, TyId)>, String)> {
    let text = result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == file)?
        .text
        .clone();
    let entries = result
        .expr_types
        .iter()
        .find(|(p, _)| p.to_string_lossy().replace('\\', "/") == file)
        .map(|(_, e)| e.clone());
    entries.map(|e| (e, text))
}

#[test]
fn unit_sources_maps_project_units_excluding_synthetic() {
    // ADR 0095: the analysis exposes a unit→source map. The clean fixture has a
    // `commons shop.util` and a `context billing.charge`.
    let result = bynk_ide::diagnose_project(
        &fixture_root("clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("clean")).to_path_buf(),
        )),
    );
    let rel = |unit: &str| -> String {
        result.unit_sources[unit][0]
            .to_string_lossy()
            .replace('\\', "/")
    };
    assert_eq!(rel("shop.util"), "shop/util.bynk");
    assert_eq!(rel("billing.charge"), "billing/charge.bynk");
    // The synthetic `bynk` surface has no openable file — excluded from the map.
    assert!(
        !result.unit_sources.contains_key("bynk"),
        "synthetic surface excluded: {:?}",
        result.unit_sources.keys().collect::<Vec<_>>()
    );
}

#[test]
fn a_clean_file_records_its_receiver_types() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("clean")).to_path_buf(),
        )),
    );
    let (entries, text) = types_for(&result, "shop/util.bynk").expect("clean file recorded");
    assert!(!entries.is_empty(), "clean file has expression types");

    // The receiver-typing use case: `xs` in `xs.fold(…)` is `List[Int]`.
    let recv = text.find("xs.fold").expect("fixture has `xs.fold`");
    assert_eq!(
        type_at_offset(&entries, recv, &result.ty_intern)
            .map(|t| result.ty_intern.get(t))
            .as_deref(),
        Some(&Ty::List(result.ty_intern.intern(Ty::Base(BaseType::Int)))),
        "the `.fold` receiver types as List[Int]"
    );
}

#[test]
fn an_erroring_file_still_records_its_well_typed_expressions() {
    // ADR 0094 (lifting ADR 0063's ceiling): the broken fixture's `bad` fn has a
    // type error, so `check_record` returns `Err` — but the well-typed
    // expressions elsewhere (`good`'s `n * 2`) are still captured in Analyse mode,
    // so `.`-member completion / signature help work on a buffer with an unrelated
    // error. The diagnostic is still reported.
    let result = bynk_ide::diagnose_project(
        &fixture_root("broken"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("broken")).to_path_buf(),
        )),
    );
    assert!(
        result
            .files
            .iter()
            .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == "billing/charge.bynk")
            .is_some_and(|f| !f.diagnostics.is_empty()),
        "the broken fixture still carries its error"
    );
    let (entries, text) = types_for(&result, "billing/charge.bynk")
        .expect("ADR 0094: an erroring file now records its partial types");
    // A well-typed receiver away from the error (`n` in `good`'s `n * 2`) types as
    // `Int` — so typing `n.` there would complete despite `bad` failing to check.
    let off = text.find("n * 2").expect("fixture has `n * 2` in `good`");
    assert_eq!(
        type_at_offset(&entries, off, &result.ty_intern)
            .map(|t| result.ty_intern.get(t))
            .as_deref(),
        Some(&Ty::Base(BaseType::Int)),
        "`good`'s expressions are typed even though `bad` errors"
    );
}

#[test]
fn an_erroring_handler_body_records_its_well_typed_receivers() {
    // ADR 0094: handler bodies are typed in `check_context_declarations`, a later
    // exit than `check_record`. Here `check_record` is clean (no top-level fns)
    // but the handler errors (`cents + true`); the receiver `cents` still types as
    // `Int`, recorded at the declaration-check exit.
    let result = bynk_ide::diagnose_project(
        &fixture_root("broken_handler"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("broken_handler")).to_path_buf(),
        )),
    );
    assert!(
        result
            .files
            .iter()
            .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == "billing/handler.bynk")
            .is_some_and(|f| !f.diagnostics.is_empty()),
        "the handler fixture carries its error"
    );
    let (entries, text) = types_for(&result, "billing/handler.bynk")
        .expect("ADR 0094: an erroring handler body still records its partial types");
    let off = text
        .find("cents + true")
        .expect("fixture has `cents + true`");
    assert_eq!(
        type_at_offset(&entries, off, &result.ty_intern)
            .map(|t| result.ty_intern.get(t))
            .as_deref(),
        Some(&Ty::Base(BaseType::Int)),
        "the receiver `cents` types as Int even though the handler body errors"
    );
}
