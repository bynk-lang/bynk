//! P4.1 (#1115) built this as a differential fixture proving this crate's
//! `bynk-check`-native analysis entry point
//! ([`bynk_check::analysis::analyse_project`]) produced the same diagnostics
//! as `bynk-emit`'s `analyse_project_with` (which drove `run_checks`'s
//! `Mode::Analyse` arm). #1541 deleted `analyse_project_with` — it had no
//! caller left once `bynk-ide` was repointed at `analyse_project` (P4.2) —
//! and `bynk-check`'s dev-dependency on `bynk-emit` along with it. The
//! comparison is gone; the fixture shape (which deliberately avoids most of
//! the residual-gap categories below, on purpose) is worth keeping, so this
//! is now a golden test of `analyse_project` alone.
//!
//! ## Why the fixture avoids the residual-gap categories
//!
//! `bynk_check::analysis::analyse_project`'s own doc comment names seven
//! categories of whole-project checking it once diverged from `bynk-emit`'s
//! `run_checks` on. All seven have since closed (both paths called the same
//! relocated `bynk_check::project_model`/`bynk_check::test_suites` functions
//! by the end), so there is no longer a legitimate divergence for a fixture
//! to dodge — but the clean/broken fixtures below predate every one of those
//! closures and there's no reason to reshape them now: no `messages` block,
//! no `Locale` capability consumption, no `Events`/`from Events(...)`
//! subscription, nothing that reaches a platform-native capability (no
//! `--platform` lock to trip), no generic-record boundary violation (a
//! function type in a non-boundary position), and no `test`/`test
//! integration` block. `new_entry_point_reports_a_function_type_boundary_violation`
//! and `new_entry_point_reports_a_test_bodys_own_type_error` below cover the
//! two categories (6 and 7) a dedicated fixture was needed for.
//!
//! What the shared fixture *does* exercise: a commons (`uses`-imported into
//! the context, so `compose_unit_symbols`'s mixin path runs), a
//! locally-declared capability with a Bynk-bodied provider
//! (`phase_validate_providers`'s op-matching check, plus
//! `check_context_declarations`'s provider/service wiring), and an HTTP
//! handler with `by`/`given` clauses (`resolver`/`checker`'s normal per-file
//! path plus the context-checks transitive closure).

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_check::analysis;
use bynk_project::{AttributedError, Roots, SchemaLock};

const SHARED_SRC: &str = r#"commons demo.shared

type Widget = {
  id:    String,
  count: Int,
}
"#;

const CONTEXT_SRC: &str = r#"context demo.svc

uses demo.shared

capability Counter {
  fn next(seed: Int) -> Effect[Int]
  fn reset() -> Effect[()]
}

provides Counter = StubCounter {
  fn next(seed: Int) -> Effect[Int] {
    Effect.pure(seed + 1)
  }

  fn reset() -> Effect[()] {
    Effect.pure(())
  }
}

service api from http {
  on GET("/widgets/:id") (id: String) -> Effect[HttpResult[Widget]] by Visitor given Counter {
    let n <- Counter.next(1)
    Ok(Widget { id: id, count: n })
  }
}
"#;

/// A self-contained temp project (real files on disk — discovery walks the
/// filesystem, so an overlay alone can't stand in for file *existence*).
/// Same convention as `bynk-ide`'s own `wire_contract.rs`/`sequence.rs` test
/// helpers: a name-scoped temp dir, cleaned up on entry, never `examples/`.
///
/// Also returns a *complete* overlay for every file written — the
/// content-ownership track (#1086, slice 5) removed `read_source`'s disk-read
/// fallback, so `bynk-project`'s discovery now errors (`no overlay entry
/// for ...`) on any file the overlay doesn't cover; production callers get a
/// complete overlay from `bynk-testkit`/`bynk-ide::discover_files`, and this
/// test builds the same completeness by hand from the files it just wrote.
fn setup_project(test_name: &str, files: &[(&str, &str)]) -> (PathBuf, HashMap<PathBuf, String>) {
    let root = std::env::temp_dir().join(format!(
        "bynk-check-differential-analysis-{test_name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test root");
    let mut overlay = HashMap::new();
    for (rel, contents) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&p, contents).expect("write file");
        overlay.insert(p, (*contents).to_string());
    }
    (root, overlay)
}

/// `AttributedError` rendered to a comparable, `Eq`-able form: the
/// project-relative path (if any), the diagnostic category, and the
/// rendered message. Span/label/note/suggestion positions are intentionally
/// excluded — pinning category+message+path already pins the diagnostic;
/// comparing spans too would just make the assertion more brittle to
/// reformat without adding confidence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RenderedError {
    path: Option<String>,
    category: &'static str,
    message: String,
}

fn render(errors: &[AttributedError]) -> Vec<RenderedError> {
    errors
        .iter()
        .map(|a| RenderedError {
            path: a
                .source_path
                .as_ref()
                .map(|p| p.to_string_lossy().replace('\\', "/")),
            category: a.error.category,
            message: a.error.message.clone(),
        })
        .collect()
}

/// Sorted, forward-slashed keys of a `PathBuf`-keyed map — for pinning a
/// `ProjectAnalysis` field's file coverage without depending on platform
/// path-separator rendering.
fn sorted_path_keys<V>(m: &HashMap<PathBuf, V>) -> Vec<String> {
    let mut ks: Vec<String> = m
        .keys()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    ks.sort();
    ks
}

fn sorted_keys<K: Ord + Clone, V>(m: &HashMap<K, V>) -> Vec<K> {
    let mut ks: Vec<K> = m.keys().cloned().collect();
    ks.sort();
    ks
}

/// Expected file/unit coverage for [`assert_structural_coverage`] — grouped
/// into a struct rather than seven parameters (clippy's `too_many_arguments`).
struct ExpectedCoverage<'a> {
    hints: &'a [&'a str],
    locals: &'a [&'a str],
    expr_types: &'a [&'a str],
    requirements: &'a [&'a str],
    sequence_info: &'a [&'a str],
    boundary_info: &'a [&'a str],
    doc_scope: &'a [&'a str],
}

/// Structural parity beyond `errors`: the ten other `ProjectAnalysis` fields
/// `bynk-ide` reads and P4.2 exists to repoint — review feedback on #1117 (a
/// fixture that pins two of twelve fields "cannot observe" a divergence in
/// the other ten by construction). #1541 retired the differential this
/// originally checked against `bynk-emit`'s `analyse_project_with`; these are
/// now literal golden pins on `analyse_project` alone, not a comparison.
/// `ty_intern` stays excluded: it's an opaque per-call intern table (`Types`
/// exposes no length accessor), and `expr_types`'s key set (pinned here)
/// already carries the signal a length would.
fn assert_structural_coverage(a: &analysis::ProjectAnalysis, expected: &ExpectedCoverage) {
    assert_eq!(
        sorted_path_keys(&a.hints),
        expected.hints,
        "hints' file coverage"
    );
    assert_eq!(
        sorted_path_keys(&a.locals),
        expected.locals,
        "locals' file coverage"
    );
    assert_eq!(
        sorted_path_keys(&a.expr_types),
        expected.expr_types,
        "expr_types' file coverage"
    );
    assert_eq!(
        sorted_path_keys(&a.requirements),
        expected.requirements,
        "requirements' file coverage"
    );
    assert_eq!(
        sorted_keys(&a.sequence_info),
        expected.sequence_info,
        "sequence_info's unit coverage"
    );
    assert_eq!(
        sorted_keys(&a.boundary_info),
        expected.boundary_info,
        "boundary_info's unit coverage"
    );
    assert_eq!(
        sorted_keys(&a.doc_scope),
        expected.doc_scope,
        "doc_scope's unit coverage"
    );
    assert_eq!(a.index.foreign_refs.len(), 0, "index.foreign_refs");
    assert_eq!(a.index.refinements.len(), 0, "index.refinements");
}

#[test]
fn new_entry_point_reports_no_errors_on_a_clean_project() {
    let (root, overlay) = setup_project(
        "clean",
        &[
            ("demo/shared.bynk", SHARED_SRC),
            ("demo/svc.bynk", CONTEXT_SRC),
        ],
    );
    let roots = Roots::Single(root);

    let analysed = analysis::analyse_project(&roots, &overlay, &SchemaLock::Off);

    assert!(
        analysed.errors.is_empty(),
        "fixture must be clean: {:?}",
        render(&analysed.errors)
    );

    let mut units: Vec<&String> = analysed.unit_sources.keys().collect();
    units.sort();
    assert_eq!(units, vec!["demo.shared", "demo.svc"]);
    assert_eq!(analysed.snapshots.len(), 2);
    assert_eq!(analysed.index.symbols.len(), 8);
    assert_eq!(analysed.index.calls.len(), 1);
    assert_eq!(analysed.index.impls.len(), 1);

    assert_structural_coverage(
        &analysed,
        &ExpectedCoverage {
            hints: &["demo/svc.bynk"],
            locals: &["demo/svc.bynk"],
            expr_types: &["demo/shared.bynk", "demo/svc.bynk"],
            requirements: &["demo/svc.bynk"],
            sequence_info: &["demo.svc"],
            boundary_info: &["demo.svc"],
            doc_scope: &["demo.shared", "demo.svc"],
        },
    );
}

/// Same fixture, but with an obvious semantic error (an unknown `given`
/// capability).
#[test]
fn new_entry_point_reports_errors_on_a_broken_project() {
    let broken_context = CONTEXT_SRC.replace("given Counter", "given NotACapability");
    let (root, overlay) = setup_project(
        "broken",
        &[
            ("demo/shared.bynk", SHARED_SRC),
            ("demo/svc.bynk", &broken_context),
        ],
    );
    let roots = Roots::Single(root);

    let analysed = analysis::analyse_project(&roots, &overlay, &SchemaLock::Off);

    let mut rendered = render(&analysed.errors);
    rendered.sort();
    // Sorted by (path, category, message) — all three share `demo/svc.bynk`,
    // so this is alphabetical by category.
    assert_eq!(
        rendered,
        vec![
            RenderedError {
                path: Some("demo/svc.bynk".to_string()),
                category: "bynk.given.undeclared_capability",
                message: "capability `Counter` is used but not listed in the handler's \
                          `given` clause"
                    .to_string(),
            },
            RenderedError {
                path: Some("demo/svc.bynk".to_string()),
                category: "bynk.given.unknown_capability",
                message: "capability `NotACapability` is not declared in this context".to_string(),
            },
            RenderedError {
                path: Some("demo/svc.bynk".to_string()),
                category: "bynk.given.unused_capability",
                message: "capability `NotACapability` is declared in `given` but never \
                          used in the body"
                    .to_string(),
            },
            // #1663: no `unknown name `n`` echo. The failed `let n <- …` still
            // binds `n` (error-typed), so its use in the body raises nothing.
        ],
        "renaming the `given` capability must report exactly this set of downstream errors"
    );

    // No hints: `hints::record`'s binding sites only fire on a clean
    // type-check (ADR 0056); every unit here has at least one error.
    assert_structural_coverage(
        &analysed,
        &ExpectedCoverage {
            hints: &[],
            locals: &["demo/svc.bynk"],
            expr_types: &["demo/shared.bynk", "demo/svc.bynk"],
            requirements: &["demo/svc.bynk"],
            sequence_info: &["demo.svc"],
            boundary_info: &["demo.svc"],
            doc_scope: &["demo.shared", "demo.svc"],
        },
    );
}

/// Category 6 (`design/tracks/semantics-in-the-checker.md` §6,
/// function-type-boundary checks). Same source as
/// `bynk-lsp/tests/analysis_residual_gap.rs`'s
/// `function_type_boundary_diagnostic_present` (copied from
/// `bynk-tests/tests/fixtures/negative/152_fn_type_in_service_sig`), the analogous
/// pin one layer up, at the `bynk-ide`/`bynk-lsp` call path.
#[test]
fn new_entry_point_reports_a_function_type_boundary_violation() {
    const API: &str = "context hof.api\n\nservice runner {\n  on call(f: Int -> Int) -> Effect[Int] {\n    Effect.pure(0)\n  }\n}\n";
    let (root, overlay) = setup_project("fn_boundary", &[("src/hof/api.bynk", API)]);
    let roots = Roots::Single(root);

    let analysed = analysis::analyse_project(&roots, &overlay, &SchemaLock::Off);
    let rendered = render(&analysed.errors);

    assert!(
        rendered
            .iter()
            .any(|e| e.category == "bynk.types.function_at_boundary"),
        "fixture must trigger the boundary check: {rendered:?}"
    );
}

/// Category 7 (test/integration-suite processing,
/// `design/tracks/semantics-in-the-checker.md` §6): `analyse_project` calls
/// `bynk_check::test_suites::phase_test_bodies` unconditionally, so a test
/// body's own type error is reported and its binding refs recorded — the
/// concrete go-to-definition-inside-a-test-file regression this fixture
/// originally caught as a divergence from `bynk-emit`'s `analyse_project_with`
/// (see #1541's git history for that comparison; `bynk-emit`'s own
/// `check_project_reports_a_test_body_error_past_an_earlier_structural_error`
/// is the analogous pin for `bynk-emit`'s own `run_checks` path).
#[test]
fn new_entry_point_reports_a_test_bodys_own_type_error() {
    const MATH_SRC: &str = "commons demo.math\n\nfn double(n: Int) -> Int { n * 2 }\n";
    const TEST_SRC: &str = "suite demo.math\n\ncase \"broken\" {\n  let x: Int = \"not an int\"\n  let y = double(1)\n  expect y == 2\n}\n";

    let (root, overlay) = setup_project(
        "test_body_gap",
        &[
            ("demo/math.bynk", MATH_SRC),
            ("tests/math_test.bynk", TEST_SRC),
        ],
    );
    let roots = Roots::Single(root);

    let analysed = analysis::analyse_project(&roots, &overlay, &SchemaLock::Off);

    let categories: Vec<&str> = analysed.errors.iter().map(|a| a.error.category).collect();
    assert!(
        categories.contains(&"bynk.types.let_annotation_mismatch"),
        "the test body's own type error must be reported: {categories:?}"
    );

    // The test file's `double(1)` call is a binding edge (a ref on
    // `double`'s index entry) — `phase_test_bodies` populates `&mut RefSink`
    // for it, so go-to-definition works from inside a test file too.
    let total_refs: usize = analysed.index.symbols.values().map(|e| e.refs.len()).sum();
    assert_eq!(
        total_refs, 1,
        "the test body's call to `double` must be recorded as a binding edge"
    );
}

/// #2081: under the caller's `SchemaLock::On`, `analyse_project` reconciles
/// events against the lock exactly as `run_checks` does — a non-additive
/// change and a corrupt lock are both reported, as `bynk check` reports them.
/// Under `Off`, or with no lock yet, neither can appear.
#[test]
fn new_entry_point_reconciles_events_against_the_callers_lock() {
    const EVENT_SRC: &str = r#"context commerce.order

consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
  region: String,
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId, region: "US" })
  }
}
"#;
    const LOCK: &str = r#"version = 1

[events."commerce.order.PaymentConfirmed"]
schema = 1

[[events."commerce.order.PaymentConfirmed".fields]]
name = "orderId"
type = "String"
default = false
"#;
    let (root, overlay) = setup_project("schema-lock", &[("commerce/order.bynk", EVENT_SRC)]);
    let roots = Roots::Single(root);
    let categories = |lock: &SchemaLock| -> Vec<&'static str> {
        analysis::analyse_project(&roots, &overlay, lock)
            .errors
            .iter()
            .map(|e| e.error.category)
            .collect()
    };

    let on = categories(&SchemaLock::On {
        existing: Some(LOCK.to_string()),
    });
    assert!(
        on.contains(&"bynk.event.non_additive_schema_change"),
        "a non-additive change against the lock must be reported: {on:?}"
    );

    let corrupt = categories(&SchemaLock::On {
        existing: Some(String::new()),
    });
    assert!(
        corrupt.contains(&"bynk.project.schema_registry_corrupt"),
        "a corrupt lock must be reported: {corrupt:?}"
    );

    for (what, lock) in [
        ("Off", SchemaLock::Off),
        ("a fresh project", SchemaLock::On { existing: None }),
    ] {
        let found = categories(&lock);
        assert!(
            !found
                .iter()
                .any(|c| c.starts_with("bynk.event.") || c.ends_with("_corrupt")),
            "{what} has no lock to disagree with: {found:?}"
        );
    }
}
