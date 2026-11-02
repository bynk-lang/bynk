//! v0.24 (ADR 0052): project-wide diagnostics — non-bailing, overlay-aware,
//! file-attributed. The before/after this proves: the context-file error
//! below is handler-level (`bynk.given.unused_capability`), which the old
//! single-file `diagnose` cannot produce at all (it resolves/checks
//! `Commons` units only).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diagnose/clean_and_broken/src")
}

#[test]
fn context_diagnostic_is_attributed_to_its_file() {
    let result = bynk_ide::diagnose_project(
        &fixture_root(),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root()).to_path_buf(),
        )),
    );

    // Every discovered file appears, clean ones with an empty list.
    let by_path: HashMap<_, _> = result
        .files
        .iter()
        .map(|f| (f.source_path.to_string_lossy().replace('\\', "/"), f))
        .collect();
    let broken = by_path
        .get("billing/charge.bynk")
        .expect("context file analysed");
    let clean = by_path
        .get("shop/util.bynk")
        .expect("clean commons analysed");

    assert!(
        broken
            .diagnostics
            .iter()
            .any(|d| d.error.category == "bynk.given.unused_capability"),
        "the handler-level diagnostic is attributed to the context file; got: {:?}",
        broken
            .diagnostics
            .iter()
            .map(|d| d.error.category)
            .collect::<Vec<_>>()
    );
    assert!(
        clean.diagnostics.is_empty(),
        "the clean file carries no diagnostics"
    );
    // The snapshot is the analysed text.
    assert!(broken.text.contains("given Clock"));
}

#[test]
fn overlay_changes_diagnostics_without_touching_disk() {
    // Fix the unused capability in the overlay only: the buffer USES Clock.
    let fixed = "\
context billing.charge

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = FixedClock {
  fn now() -> Effect[Int] {
    42
  }
}

service charge {
  on call(cents: Int) -> Effect[Result[Int, ()]] given Clock {
    let t <- Clock.now()
    Ok(cents + t)
  }
}
";
    let abs = fixture_root().join("billing/charge.bynk");
    let canonical = abs.canonicalize().unwrap_or(abs);
    let mut overlay = HashMap::new();
    overlay.insert(canonical, fixed.to_string());

    let result = bynk_ide::diagnose_project(&fixture_root(), &overlay);
    let broken = result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == "billing/charge.bynk")
        .expect("context file analysed");
    assert!(
        broken.diagnostics.is_empty(),
        "overlay fix clears the diagnostic; got: {:?}",
        broken
            .diagnostics
            .iter()
            .map(|d| d.error.category)
            .collect::<Vec<_>>()
    );
    // The snapshot reflects the overlay, not the disk.
    assert!(broken.text.contains("Clock.now()"));
}

#[test]
fn analysis_does_not_bail_at_the_first_failure() {
    // Break BOTH files via overlays: a parse error in one, a semantic error
    // in the other. The old compile_project contract reports only the first
    // phase's failure; diagnose_project must report both, each attributed.
    let root = fixture_root();
    let mut overlay =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    overlay.insert(
        root.join("shop/util.bynk"),
        "commons shop.util\n\nfn broken( -> Int {\n  1\n}\n".to_string(),
    );

    let result = bynk_ide::diagnose_project(&root, &overlay);
    let cats: Vec<(String, &'static str)> = result
        .files
        .iter()
        .flat_map(|f| {
            let p = f.source_path.to_string_lossy().replace('\\', "/");
            f.diagnostics
                .iter()
                .map(move |d| (p.clone(), d.error.category))
        })
        .collect();
    assert!(
        cats.iter()
            .any(|(p, c)| p == "shop/util.bynk" && c.starts_with("bynk.parse")),
        "parse error attributed to the broken commons; got {cats:?}"
    );
    assert!(
        cats.iter()
            .any(|(p, c)| p == "billing/charge.bynk" && *c == "bynk.given.unused_capability"),
        "semantic error still produced for the other unit; got {cats:?}"
    );
}

/// #485: a multi-file commons analysed via its `src/` tree resolves types
/// declared in *sibling* files — the LSP's `diagnose_project` over the same
/// root the compiler uses (legacy single-tree mode, no `bynk.toml`) must not
/// report the sibling-blind false positives single-file `diagnose` emits.
///
/// Fixture 252 declares `type Rate` in `shipping/rates/base.bynk` and calls
/// `Rate.of` from a method attached in the sibling `shipping/rates/make.bynk`
/// (same `commons shipping.rates`). Single-file mode flags `Rate` as an
/// unknown type (`bynk.resolve.method_unknown_type`); project mode resolves it.
#[test]
fn multi_file_commons_src_tree_analyses_clean() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/252_multi_file_commons_dotted_test/src");

    let result = bynk_ide::diagnose_project(
        &src,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (src).to_path_buf(),
        )),
    );

    // Both sibling files are discovered and analysed.
    let seen: Vec<String> = result
        .files
        .iter()
        .map(|f| f.source_path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        seen.iter().any(|p| p == "shipping/rates/make.bynk"),
        "make.bynk analysed; got {seen:?}"
    );

    // No file — make.bynk in particular — carries the sibling-blind false
    // positive, and nothing else is flagged either (252 is a passing fixture).
    let diags: Vec<(String, &'static str)> = result
        .files
        .iter()
        .flat_map(|f| {
            let p = f.source_path.to_string_lossy().replace('\\', "/");
            f.diagnostics
                .iter()
                .map(move |d| (p.clone(), d.error.category))
        })
        .collect();
    assert!(
        !diags
            .iter()
            .any(|(_, c)| *c == "bynk.resolve.method_unknown_type"),
        "no unknown-type false positive across the commons; got {diags:?}"
    );
    assert!(
        result.files.iter().all(|f| f.diagnostics.is_empty()),
        "the whole multi-file commons is clean; got {diags:?}"
    );
    assert!(
        result.unattributed.is_empty(),
        "no project-level diagnostics; got {:?}",
        result
            .unattributed
            .iter()
            .map(|d| d.error.category)
            .collect::<Vec<_>>()
    );
}

/// #1710: on the project path, a file that fails the strict parse still has its
/// surviving declarations checked, and a reference to a declaration recovery
/// had to skip (from a file that can see it) is a known name, not an unknown
/// one. Pinned as the exact set, per file:
/// - `shapes.bynk` (broken `type Pair` and method `Box.scale`): its syntax
///   errors, and the type error in the surviving `fn wrongType`;
/// - `vault.bynk` (broken `service open`): its syntax error, and the type error
///   in the surviving `service count`;
/// - `app.bynk` (`uses demo.shapes`): no echo for `Pair`, nor for the call to
///   the skipped `Box.scale` (`types.method_not_found`), its own type error,
///   and a genuinely unknown `Ghost` still reported;
/// - `guard.bynk` (broken `capability Audit`, `actor User`): its syntax errors,
///   and no echo from the provider, the `given`, or the `by` that name them;
/// - `headless.bynk` (no unit survives recovery): its syntax error, still at
///   its real position;
/// - `desk.bynk` (`consumes demo.vault`): no echo for the skipped `open`;
/// - `other.bynk` (sees no `demo.shapes`): `Pair` is still unknown there, so
///   the suppression is scoped to what a unit can see.
#[test]
fn partial_parse_checks_surviving_declarations_without_echoes() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diagnose/partial_parse/src");
    let result = bynk_ide::diagnose_project(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    let mut got: Vec<(String, String, String)> = result
        .files
        .iter()
        .flat_map(|f| {
            let path = f.source_path.to_string_lossy().replace('\\', "/");
            f.diagnostics.iter().map(move |d| {
                (
                    path.clone(),
                    d.error.category.to_string(),
                    d.error.message.clone(),
                )
            })
        })
        .collect();
    got.sort();
    let mut want: Vec<(String, String, String)> = [
        (
            "demo/shapes.bynk",
            "bynk.parse.expected_token",
            "expected `}` to close the record body, found identifier",
        ),
        (
            "demo/shapes.bynk",
            "bynk.parse.expected_expression",
            "expected an expression, found `}`",
        ),
        (
            "demo/shapes.bynk",
            "bynk.types.return_mismatch",
            "function body has type `String`, but the declared return type is `Int`",
        ),
        (
            "demo/vault.bynk",
            "bynk.parse.expected_expression",
            "expected an expression, found `}`",
        ),
        (
            "demo/vault.bynk",
            "bynk.types.return_mismatch",
            "handler body has type `String`, but the declared return type is `Effect[Int]`",
        ),
        (
            "demo/app.bynk",
            "bynk.types.return_mismatch",
            "function body has type `Int`, but the declared return type is `String`",
        ),
        (
            "demo/app.bynk",
            "bynk.resolve.unknown_type",
            "unknown type `Ghost`",
        ),
        (
            "demo/guard.bynk",
            "bynk.parse.expected_token",
            "expected `]` to close the `Effect` type argument, found `}`",
        ),
        (
            "demo/guard.bynk",
            "bynk.parse.expected_token",
            "expected a string or integer scheme config value",
        ),
        (
            "demo/headless.bynk",
            "bynk.parse.reserved_keyword",
            "expected identifier for the commons name, but `fn` is a reserved keyword",
        ),
        (
            "demo/other.bynk",
            "bynk.resolve.unknown_type",
            "unknown type `Pair`",
        ),
    ]
    .into_iter()
    .map(|(p, c, m)| (p.to_string(), c.to_string(), m.to_string()))
    .collect();
    want.sort();
    assert_eq!(got, want);
}

/// #2807: a lexical error no longer blanks its file. `a.bynk` holds a stray
/// `$` and, beyond it, a type error; both are reported, with nothing echoed
/// from the skipped character. The sibling file's own error is reported as
/// ever.
#[test]
fn a_lex_error_is_reported_with_the_rest_of_its_files_diagnostics() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diagnose/lex_error/src");
    let result = bynk_ide::diagnose_project(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    let mut got: Vec<(String, String)> = result
        .files
        .iter()
        .flat_map(|f| {
            let path = f.source_path.to_string_lossy().replace('\\', "/");
            f.diagnostics
                .iter()
                .map(move |d| (path.clone(), d.error.category.to_string()))
        })
        .collect();
    got.sort();
    let want: Vec<(String, String)> = [
        ("demo/a.bynk", "bynk.lex.unexpected_character"),
        ("demo/a.bynk", "bynk.types.return_mismatch"),
        ("demo/b.bynk", "bynk.types.return_mismatch"),
    ]
    .iter()
    .map(|(p, c)| (p.to_string(), c.to_string()))
    .collect();
    assert_eq!(got, want);
}
