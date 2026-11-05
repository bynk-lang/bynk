//! v0.27 (ADR 0056): the harvested inlay-hint set — inferred-type hints for
//! annotation-absent `let` / `let <-` bindings and lambda parameters,
//! per-file, labels in Bynk surface syntax, surviving a transient type
//! error at the sites the checker still reaches.

use std::path::{Path, PathBuf};

fn fixture_root(which: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/inlay")
        .join(which)
        .join("src")
}

/// The fixture file's hints alongside its analysed text.
fn hints_for(result: &bynk_ide::ProjectDiagnostics, file: &str) -> (Vec<(usize, String)>, String) {
    let text = result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == file)
        .unwrap_or_else(|| panic!("{file} analysed"))
        .text
        .clone();
    let hints = result
        .hints
        .iter()
        .find(|(p, _)| p.to_string_lossy().replace('\\', "/") == file)
        .map(|(_, hs)| {
            hs.iter()
                // This harness exercises inferred-type hints; parameter-name
                // hints (v0.39) are checked separately.
                .filter(|h| h.kind == bynk_check::hints::HintKind::Type)
                .map(|h| (h.span.start, h.label.clone()))
                .collect()
        })
        .unwrap_or_default();
    (hints, text)
}

/// The hint label at binding `name` (matched at the byte offset of
/// `pattern`'s first occurrence plus `pattern.find(name)`).
fn label_at<'a>(
    hints: &'a [(usize, String)],
    text: &str,
    pattern: &str,
    name: &str,
) -> Option<&'a str> {
    let line = text
        .find(pattern)
        .unwrap_or_else(|| panic!("fixture contains `{pattern}`"));
    let offset = line
        + pattern
            .find(name)
            .unwrap_or_else(|| panic!("`{pattern}` contains `{name}`"));
    hints
        .iter()
        .find(|(start, _)| *start == offset)
        .map(|(_, l)| l.as_str())
}

/// v0.39: a fixture file's parameter-name hints as `(arg-start offset, label)`.
fn param_hints_for(
    result: &bynk_ide::ProjectDiagnostics,
    file: &str,
) -> (Vec<(usize, String)>, String) {
    let text = result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == file)
        .unwrap_or_else(|| panic!("{file} analysed"))
        .text
        .clone();
    let hints = result
        .hints
        .iter()
        .find(|(p, _)| p.to_string_lossy().replace('\\', "/") == file)
        .map(|(_, hs)| {
            hs.iter()
                .filter(|h| h.kind == bynk_check::hints::HintKind::Parameter)
                .map(|h| (h.span.start, h.label.clone()))
                .collect()
        })
        .unwrap_or_default();
    (hints, text)
}

#[test]
fn parameter_name_hints_show_at_args_and_suppress_matching_identifiers() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("params"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("params")).to_path_buf(),
        )),
    );
    let (hints, text) = param_hints_for(&result, "demo.bynk");

    let call = text.find("area(w, height)").expect("the call is present");
    // `width:` shows before the `w` argument (w != the param `width`).
    let w_off = call + "area(".len();
    assert_eq!(
        hints
            .iter()
            .find(|(o, _)| *o == w_off)
            .map(|(_, l)| l.as_str()),
        Some("width:")
    );
    // `height:` is suppressed — the argument is the identically-named `height`.
    let height_off = call + "area(w, ".len();
    assert!(
        hints.iter().all(|(o, _)| *o != height_off),
        "the identically-named `height` argument gets no hint"
    );
}

#[test]
fn generic_instantiation_hints_show_inferred_args_only_when_omitted() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("generics"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("generics")).to_path_buf(),
        )),
    );
    // Generic-instantiation hints are `Type`-kind, anchored on the fn name.
    let (hints, text) = hints_for(&result, "demo.bynk");

    // `identity(5)` → the inferred `[Int]` shown after the name.
    assert_eq!(
        label_at(&hints, &text, "identity(5)", "identity"),
        Some("[Int]")
    );
    // `identity[Int](5)` → the user wrote the args, so no hint.
    assert_eq!(
        label_at(&hints, &text, "identity[Int](5)", "identity"),
        None
    );
}

#[test]
fn let_bindings_and_lambda_params_get_inferred_type_hints() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("clean")).to_path_buf(),
        )),
    );
    let (hints, text) = hints_for(&result, "shop/util.bynk");

    // `let =` with an inferred type — the headline.
    assert_eq!(
        label_at(&hints, &text, "let total = ", "total"),
        Some(": Int")
    );
    // Lambda params typed from `fold`'s expected fn type.
    assert_eq!(
        label_at(&hints, &text, "(acc, x) => ", "acc"),
        Some(": Int")
    );
    assert_eq!(label_at(&hints, &text, "(acc, x) => ", "x"), Some(": Int"));
}

#[test]
fn labels_read_in_bynk_surface_syntax() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("clean")).to_path_buf(),
        )),
    );
    let (hints, text) = hints_for(&result, "shop/util.bynk");

    // Display fidelity: generic source syntax, not an internal rendering.
    assert_eq!(
        label_at(&hints, &text, "let xs = ", "xs"),
        Some(": List[Int]")
    );
    assert_eq!(
        label_at(&hints, &text, "let first = ", "first"),
        Some(": Option[Int]")
    );
    // The lambda-typed binding pins the `Fn` rendering.
    assert_eq!(
        label_at(&hints, &text, "let f = ", "f"),
        Some(": Int -> Int")
    );
}

#[test]
fn effect_let_hints_show_the_peeled_payload() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("clean")).to_path_buf(),
        )),
    );
    let (hints, text) = hints_for(&result, "billing/charge.bynk");

    // `let stamp <- Clock.now()` binds the Effect payload — `Int`, never
    // `Effect[Int]`.
    assert_eq!(
        label_at(&hints, &text, "let stamp <- ", "stamp"),
        Some(": Int")
    );
}

#[test]
fn annotated_and_underscore_bindings_get_no_hint() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("clean")).to_path_buf(),
        )),
    );
    let (hints, text) = hints_for(&result, "shop/util.bynk");

    // An explicit annotation needs no hint; `_` binds nothing.
    assert_eq!(label_at(&hints, &text, "let twice: Int = ", "twice"), None);
    assert_eq!(label_at(&hints, &text, "let _ = ", "_"), None);
    // An annotated lambda param gets no hint either.
    assert_eq!(label_at(&hints, &text, "(x: Int) => ", "x"), None);
}

#[test]
fn clean_fixture_has_no_diagnostics() {
    // The hint fixtures must be diagnostically clean, or the other tests
    // assert against a half-checked project.
    let result = bynk_ide::diagnose_project(
        &fixture_root("clean"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("clean")).to_path_buf(),
        )),
    );
    for f in &result.files {
        assert!(
            f.diagnostics.is_empty(),
            "{}: {:?}",
            f.source_path.display(),
            f.diagnostics
                .iter()
                .map(|d| d.error.category)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn hints_survive_a_transient_error_at_reached_sites() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("broken"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("broken")).to_path_buf(),
        )),
    );
    let (hints, text) = hints_for(&result, "billing/charge.bynk");

    // The file has one fn-body type error (`n + true` in `bad`)...
    assert!(
        result
            .files
            .iter()
            .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == "billing/charge.bynk")
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| d.error.category.starts_with("bynk.types.")),
        "the broken fixture carries its type error"
    );
    // ...but the erroring binding's sibling fn still hints: the sink is a
    // `&mut` parameter, not part of the Ok payload `check_record` drops.
    assert_eq!(label_at(&hints, &text, "let m = ", "m"), Some(": Int"));
    // The erroring binding itself has no computed type, so no hint.
    assert_eq!(label_at(&hints, &text, "let s = ", "s"), None);
    // #1663 (Decision A): a fn-body error no longer short-circuits the v0.5
    // pass, so the handler body is still typed and its `let <-` hints too.
    assert_eq!(
        label_at(&hints, &text, "let stamp <- ", "stamp"),
        Some(": Int")
    );
}

/// #2727: a binding whose value only faults is refused, and no hint offers the
/// bottom type (`never returns`) as source; a joined fault keeps its real type.
#[test]
fn a_never_binding_is_refused_and_offers_no_hint() {
    let result = bynk_ide::diagnose_project(
        &fixture_root("never"),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root("never")).to_path_buf(),
        )),
    );
    let (hints, text) = hints_for(&result, "demo.bynk");
    assert!(
        hints.iter().all(|(_, l)| !l.contains("never returns")),
        "{hints:?}"
    );
    assert_eq!(
        label_at(&hints, &text, "let bad =", "bad"),
        None,
        "the refused binding has no hint"
    );
    assert_eq!(label_at(&hints, &text, "let ok =", "ok"), Some(": Int"));
    assert!(
        result
            .files
            .iter()
            .flat_map(|f| f.diagnostics.iter())
            .any(|d| d.error.category == "bynk.types.never_inferred"),
        "the binding is refused"
    );
}
