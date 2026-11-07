//! Finding #64: `bynk check`'s directory path used to route through
//! `compile_project`, whose `Mode::Build` bails at the first structural error
//! and returns only what it collected up to that point. Once any file in the
//! project had an earlier structural error (here, `exports capability`
//! naming an undeclared capability), every later diagnostic — including a
//! test body's own type error, which only surfaces in the per-unit checking
//! pass that runs *after* the bail gate — was silently dropped, even though
//! the editor (`bynk_check::analysis::analyse_project`, which already used
//! this same non-bailing shape) still reported it. `bynk check` now routes
//! through `check_project`
//! (`Mode::Analyse`, never bails) instead, so both diagnostics are reported
//! and the exit code is decided by "does any error-severity diagnostic
//! exist," not "did the pipeline reach the end without bailing."

use std::path::PathBuf;
use std::process::Command;

#[test]
fn check_reports_a_test_body_error_past_an_earlier_structural_error() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/behaviour/check_reports_past_structural_error");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("check")
        .arg(&fixture)
        .output()
        .expect("run bynk check");
    assert!(
        !out.status.success(),
        "a project with a real structural error must not exit 0"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bynk.exports.undeclared_capability"),
        "expected the earlier structural error, got:\n{stderr}"
    );
    assert!(
        stderr.contains("bynk.types.let_annotation_mismatch"),
        "expected the test body's own type error to still be reported \
         past the earlier structural error, got:\n{stderr}"
    );
}
