//! Regression: `bynk.toml` is the one input a user hand-edits that the
//! compiler used to read without checking. The deleted `read_project_paths`
//! was total by construction — a syntax error or a typo'd `[paths]` key both
//! silently fell back to the conventional layout with no diagnostic and exit
//! 0, after which the user would only see a downstream cascade (e.g.
//! `bynk.uses.unknown_target` pointing at units that plainly exist on disk)
//! with no hint at the real cause. `try_read_project_paths` (R3.8, #1113)
//! surfaces both instead.
//!
//! Drives the real `bynkc check` CLI against the two malformed-config
//! fixtures and asserts each now fails loudly with an actionable message,
//! instead of silently succeeding — plus the R3.9 (#1113) counterpart: an
//! `include` list of three or more roots used to be capped at two
//! (`too_many_include_roots`, silently dropping the third tree); `Roots::trees`
//! now walks every entry, so the same shape of fixture must instead *succeed*
//! and discover all three trees.

use std::path::PathBuf;
use std::process::Command;

fn check(fixture: &str) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("check")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(fixture))
        .output()
        .expect("run bynkc check");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn a_syntactically_invalid_bynk_toml_fails_loudly() {
    let (ok, out) = check("tests/fixtures/behaviour/malformed_bynk_toml");
    assert!(!ok, "expected failure, but the build succeeded:\n{out}");
    assert!(
        out.contains("not valid TOML"),
        "expected a TOML-parse diagnostic, got:\n{out}"
    );
}

#[test]
fn a_typo_d_paths_key_fails_loudly_instead_of_being_silently_ignored() {
    let (ok, out) = check("tests/fixtures/behaviour/typo_paths_key");
    assert!(!ok, "expected failure, but the build succeeded:\n{out}");
    assert!(
        out.contains("inculde") && out.contains("include"),
        "expected the diagnostic to name the typo'd key and suggest `include`, got:\n{out}"
    );
}

/// #1665: an unknown table (here one for a planned `bynk deploy` feature) used
/// to build cleanly with none of its behaviour. It now fails, naming the issue.
#[test]
fn an_unknown_table_fails_loudly_instead_of_being_silently_ignored() {
    let (ok, out) = check("tests/fixtures/behaviour/unknown_manifest_table");
    assert!(!ok, "expected failure, but the build succeeded:\n{out}");
    assert!(
        out.contains("[deploy]") && out.contains("not yet supported (#551)"),
        "expected the table named and its issue cited, got:\n{out}"
    );
}

/// #2207 (packaging slice 1, decision A): a well-formed `[dependencies]` is
/// checked and then refused, naming the slice that resolves it, rather than
/// built while ignored.
#[test]
fn declared_dependencies_are_refused_until_they_can_be_resolved() {
    let (ok, out) = check("tests/fixtures/behaviour/declared_dependencies");
    assert!(!ok, "expected failure, but the build succeeded:\n{out}");
    assert!(
        out.contains(
            "`[dependencies]` declares dependencies, but they can't be resolved until packaging slice 2 (#843) — remove them for now"
        ),
        "got:\n{out}"
    );
}

/// #1770 review: a table's name holding a plain value (`paths = "src"`) used
/// to read as an absent `[paths]` and fall back to the conventional layout.
#[test]
fn a_table_name_with_a_plain_value_fails_loudly() {
    let (ok, out) = check("tests/fixtures/behaviour/table_name_as_plain_value");
    assert!(!ok, "expected failure, but the build succeeded:\n{out}");
    assert!(out.contains("write it as `[paths]`"), "got:\n{out}");
}

/// R3.9 (#1113): a three-entry `[paths] include` used to be rejected
/// (`ProjectPathsError::TooManyIncludeRoots`) — now every entry is a real
/// tree `Roots::trees` walks, so a well-formed three-root project must
/// compile clean, with all three trees' units visible to each other (the
/// `tests/` suite calls `demo.f`, declared in `src/`).
#[test]
fn three_or_more_include_roots_are_all_discovered() {
    let (ok, out) = check("tests/fixtures/behaviour/three_or_more_include_roots");
    assert!(ok, "expected success, but the build failed:\n{out}");
}
