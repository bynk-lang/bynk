//! Regression for ADR 0117's warning channel being only half-wired: a
//! warning-severity diagnostic printed `Error:` (`report_with_config`
//! hardcoded `ReportKind::Error`), `bynkc check --format short` on a project
//! ignored `short` entirely for a successful build, and a successful
//! project build kept no per-file snapshots so a warning had no line/col to
//! report even once `short` was consulted.
//!
//! Drives the real `bynkc check` CLI (no `tsc`/`node` dependency — `check`
//! never compiles to JS) against two fixtures: a single-file `commons` with
//! an orphaned doc block, and a project with an unused `given` capability.

use std::path::PathBuf;
use std::process::Command;

fn run_check(path: &str, extra_args: &[&str]) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"));
    cmd.arg("check")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path));
    cmd.args(extra_args);
    let out = cmd.output().expect("run bynkc check");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn single_file_warning_prints_warning_not_error_and_exits_clean() {
    let (ok, out) = run_check("tests/fixtures/behaviour/warning_severity_single.bynk", &[]);
    assert!(ok, "a warning-only build must still exit 0, got:\n{out}");
    assert!(
        out.contains("Warning:"),
        "expected a `Warning:` report, got:\n{out}"
    );
    assert!(
        !out.contains("Error:"),
        "a warning-severity diagnostic must not print `Error:`, got:\n{out}"
    );
}

#[test]
fn project_warning_format_short_consults_short_and_has_position() {
    let fixture = "tests/fixtures/behaviour/warning_severity";

    let (ok, plain) = run_check(fixture, &[]);
    assert!(
        ok,
        "a warning-only project build must exit 0, got:\n{plain}"
    );
    assert!(
        plain.contains("Warning:") && !plain.contains("Error:"),
        "expected an ariadne `Warning:` report for the project's unused `given`, got:\n{plain}"
    );

    let (ok_short, short) = run_check(fixture, &["--format", "short"]);
    assert!(ok_short, "--format short must also exit 0, got:\n{short}");
    // The exact bug: before the fix, `--format short` on a successful project
    // build printed the same position-free, multi-line plain form as the
    // default format — `run_check`'s `Ok` arm never consulted `short`.
    assert!(
        short.contains("warn_unused_given.bynk:") && short.contains(':'),
        "expected a `path:line:col: warning[...]: ...` short line, got:\n{short}"
    );
    assert_ne!(
        plain, short,
        "--format short must render differently from the default format"
    );
}
