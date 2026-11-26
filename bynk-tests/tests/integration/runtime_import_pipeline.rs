//! Which conditional runtime helpers a module's import line ends up carrying, and the import
//! collector's reach into a match-arm guard (#1832). 1B.2b (#4028): these tests drive the driver's
//! in-memory pipeline, so they moved from `bynk-emit` (`emitter.rs`).

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::{emit_bundle, emit_project};

/// The runtime import lines of the emitted module, joined: one `import …
/// from "./__bynk_runtime/<module>.js"` per module it uses.
fn runtime_import_line(ts: &str) -> String {
    ts.lines()
        .filter(|l| l.starts_with("import ") && l.contains("__bynk_runtime/"))
        .collect::<Vec<_>>()
        .join("\n")
}

// -- the ICU formatters ---------------------------------------------------

const ICU_HELPERS: [&str; 3] = ["__selectPluralArm", "__formatIcuNumber", "__formatIcuDate"];

/// The case the per-arm recording exists for. A `select` placeholder lowers to
/// `Object.hasOwn` over an arm table and calls no formatter, so a bundle whose
/// only ICU construct is a `select` must import none of the three — recording
/// once per placeholder instead of per arm would import all three here.
#[test]
fn a_select_only_bundle_imports_no_icu_formatter() {
    let ts = emit_bundle(
        "messages \"en\" @reference {\n  \"greeting\" => \"{g, select, male {He} female {She} other {They}} liked this.\"\n}\n",
    );
    assert!(
        ts.contains("globalThis.Object.hasOwn"),
        "the select arm table should have been emitted, else this proves nothing: {ts}"
    );
    for helper in ICU_HELPERS {
        assert!(
            !runtime_import_line(&ts).contains(helper),
            "a select-only bundle calls no formatter, so `{helper}` must not be imported: {ts}"
        );
    }
}

/// The opposite direction: a `plural` placeholder does call a formatter. The
/// import is what the tree spells (1A.36b-1), so the dispatch and the number
/// formatter its `#` uses come in, and the date formatter, which no arm calls,
/// does not.
#[test]
fn a_plural_bundle_imports_the_icu_formatters_it_calls() {
    let ts = emit_bundle(
        "messages \"en\" @reference {\n  \"cart\" => \"You have {n, plural, one {# item} other {# items}} in your cart\"\n}\n",
    );
    assert!(
        ts.contains("__selectPluralArm("),
        "the plural dispatch should have been emitted: {ts}"
    );
    for helper in ["__selectPluralArm", "__formatIcuNumber"] {
        assert!(
            runtime_import_line(&ts).contains(helper),
            "`{helper}` should be imported for a plural bundle: {ts}"
        );
    }
    assert!(
        !runtime_import_line(&ts).contains("__formatIcuDate"),
        "no arm formats a date: {ts}"
    );
}

/// A bundle with no ICU dispatch at all — a plain `{name}` placeholder goes
/// through `renderArg`, not a formatter.
#[test]
fn a_plain_placeholder_bundle_imports_no_icu_formatter() {
    let ts = emit_bundle("messages \"en\" @reference {\n  \"hello\" => \"Hello, {name}!\"\n}\n");
    for helper in ICU_HELPERS {
        assert!(
            !runtime_import_line(&ts).contains(helper),
            "`{helper}` must not be imported for a bundle with no ICU dispatch: {ts}"
        );
    }
}

#[test]
fn a_function_named_only_in_a_guard_is_imported() {
    let out = emit_project(
        &[
            ("d/lim.bynk", "commons d.lim\n\nfn lim() -> Int { 9 }\n"),
            (
                "d/app.bynk",
                "commons d.app\n\nuses d.lim\n\nfn g(n: Int) -> Int {\n  match n {\n    k if k > lim() => 1\n    _ => 0\n  }\n}\n",
            ),
        ],
        bynk_check::project_model::BuildTarget::Bundle,
    );
    let app = out.text("d/app.ts");
    assert!(
        app.lines()
            .any(|l| l.starts_with("import") && l.contains("lim") && l.contains("./lim.js")),
        "{app}"
    );
}
