//! v0.31 (ADR 0064): local bindings recorded with their scope ranges — the
//! scope-at-offset foundation. Covers every slice-1 binding kind (`let`/
//! `let <-`, fn/handler/lambda params) and, critically, scope correctness
//! under nesting and shadowing.

use bynk_check::locals::{LocalBinding, locals_at};
use std::path::{Path, PathBuf};

fn fixture_root(which: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(which)
        .join("src")
}

fn locals_for(result: &bynk_ide::ProjectDiagnostics, file: &str) -> (Vec<LocalBinding>, String) {
    let text = result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == file)
        .unwrap_or_else(|| panic!("{file} analysed"))
        .text
        .clone();
    let locals = result
        .locals
        .iter()
        .find(|(p, _)| p.to_string_lossy().replace('\\', "/") == file)
        .map(|(_, l)| l.clone())
        .unwrap_or_default();
    (locals, text)
}

fn names_at(locals: &[LocalBinding], offset: usize) -> Vec<String> {
    locals_at(locals, offset)
        .iter()
        .map(|b| b.name.clone())
        .collect()
}

#[test]
fn every_binding_kind_is_recorded() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("inlay/clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("inlay/clean")).to_path_buf(),
        )),
    );
    let (locals, _) = locals_for(&result, "shop/util.bynk");
    let names: std::collections::HashSet<&str> = locals.iter().map(|b| b.name.as_str()).collect();
    for want in ["n", "xs", "total", "first", "f", "twice", "acc", "x"] {
        assert!(names.contains(want), "binding `{want}` recorded: {names:?}");
    }
    // The `_` wildcard binds nothing.
    assert!(!names.contains("_"), "wildcard not recorded");
}

#[test]
fn nested_block_bindings_are_scoped_to_their_block() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("locals"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("locals")).to_path_buf(),
        )),
    );
    let (locals, text) = locals_for(&result, "m.bynk");

    // Inside the `if`-then block, `inner` is in scope (with the param `n`).
    let inner_use = text.find("inner\n  }").expect("inner use");
    let at_inner = names_at(&locals, inner_use);
    assert!(at_inner.contains(&"inner".to_string()), "{at_inner:?}");
    assert!(at_inner.contains(&"n".to_string()), "{at_inner:?}");

    // In the `else` block, `inner` has left scope; `n` is still visible.
    let else_n = text
        .find("else {\n    n")
        .map(|i| i + "else {\n    ".len())
        .expect("else n");
    let at_else = names_at(&locals, else_n);
    assert!(
        !at_else.contains(&"inner".to_string()),
        "inner is then-block-only: {at_else:?}"
    );
    assert!(at_else.contains(&"n".to_string()), "{at_else:?}");
}

#[test]
fn shadowing_resolves_to_the_latest_binding() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("locals"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("locals")).to_path_buf(),
        )),
    );
    let (locals, text) = locals_for(&result, "m.bynk");

    // `let x = n` then `let x = x + 1`; at the tail, exactly one `x` is in
    // scope — the second (latest) definition.
    let tail_x = text.rfind("\n  x\n}").map(|i| i + 3).expect("tail x");
    let xs: Vec<&LocalBinding> = locals_at(&locals, tail_x)
        .into_iter()
        .filter(|b| b.name == "x")
        .collect();
    assert_eq!(xs.len(), 1, "one `x` in scope (shadowing): {xs:?}");
    let second_let = text.rfind("let x").expect("second let x") + "let ".len();
    assert_eq!(xs[0].def_span.start, second_let, "the latest `x` wins");
}
