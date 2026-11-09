//! #3143: `scripts/ci/ci-sweep-report.sh` (the reporting half of the CI sweep)
//! opens or comments on the "CI runs missing" issue, once per SHA. A stub `gh`
//! answers the issue lookups and records the writes, so the path a real sweep
//! only takes when a run is lost is exercised here.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci/ci-sweep-report.sh")
}

/// True when `jq` is on PATH. The stub `gh` applies the script's real `--jq`
/// filters with it; Linux CI always has it, so a miss there fails loudly.
fn have_jq() -> bool {
    let ok = Command::new("jq").arg("--version").output().is_ok();
    assert!(ok || !cfg!(target_os = "linux"), "jq is required");
    ok
}

/// Runs the report with `findings` on stdin. The stub `gh` answers
/// `issue list` / `issue view` by running the `--jq` filter it was given over
/// `issues` / `view` (JSON fixtures), and records the writes. Returns
/// (stdout, the stub's call log), or None when `jq` is unavailable.
fn run(tag: &str, findings: &str, issues: &str, view: &str) -> Option<(String, String)> {
    if !have_jq() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!("bynk-sweep-report-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("issues.json"), issues).unwrap();
    std::fs::write(dir.join("view.json"), view).unwrap();
    let gh = dir.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/sh
q=""; prev=""
for a in "$@"; do [ "$prev" = "--jq" ] && q=$a; prev=$a; done
case "$1 $2" in
  "issue list") jq -r "$q" "$STUB_DIR/issues.json" ;;
  "issue view") jq -r "$q" "$STUB_DIR/view.json" ;;
  *) printf '%s\n' "$*" >> "$STUB_DIR/calls.log" ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let mut child = Command::new("bash")
        .arg(script())
        .env("PATH", path)
        .env("STUB_DIR", &dir)
        .env("GH_REPO", "o/r")
        .env("GITHUB_SERVER_URL", "https://github.test")
        .env("GITHUB_RUN_ID", "42")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(findings.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    Some((String::from_utf8_lossy(&out.stdout).into_owned(), calls))
}

const ISSUES: &str =
    r#"[{"number":5,"title":"CI runs missing (old)"},{"number":7,"title":"CI runs missing"}]"#;
const NEAR_MISS_ONLY: &str = r#"[{"number":5,"title":"CI runs missing (old)"}]"#;
const VIEW_AAA_IN_COMMENT: &str =
    r#"{"body":"tracking","comments":[{"body":"x <!-- ci-sweep:aaa -->"}]}"#;

const A: &str = "- PR #1: head `aaa` has no `ci.yml` run. <!-- ci-sweep:aaa -->\n";
const B: &str = "- PR #2: head `bbb` has no `ci.yml` run. <!-- ci-sweep:bbb -->\n";

#[test]
fn no_findings_writes_nothing() {
    let Some((out, calls)) = run("none", "", "[]", "{}") else {
        return;
    };
    assert!(out.contains("Every open PR head"));
    assert!(calls.is_empty());
}

#[test]
fn no_open_issue_creates_one_with_every_finding() {
    let Some((_, calls)) = run("create", &format!("{A}{B}"), "[]", "{}") else {
        return;
    };
    assert!(
        calls.starts_with("issue create --title CI runs missing"),
        "{calls}"
    );
    assert!(calls.contains("ci-sweep:aaa") && calls.contains("ci-sweep:bbb"));
    assert!(calls.contains("https://github.test/o/r/actions/runs/42"));
}

#[test]
fn a_near_miss_title_is_not_the_tracking_issue() {
    // "CI runs missing (old)" is not an exact match, so a new issue is opened.
    let Some((_, calls)) = run("nearmiss", A, NEAR_MISS_ONLY, "{}") else {
        return;
    };
    assert!(calls.starts_with("issue create"), "{calls}");
}

#[test]
fn an_open_issue_gets_a_comment_with_only_the_new_shas() {
    // The exact-title match picks #7 over the near miss #5; `aaa` is known from a comment.
    let Some((_, calls)) = run("comment", &format!("{A}{B}"), ISSUES, VIEW_AAA_IN_COMMENT) else {
        return;
    };
    assert!(calls.starts_with("issue comment 7 --body"), "{calls}");
    assert!(calls.contains("ci-sweep:bbb"), "{calls}");
    assert!(
        !calls.contains("ci-sweep:aaa"),
        "a known SHA is not repeated: {calls}"
    );
}

#[test]
fn already_reported_shas_write_nothing() {
    let view = r#"{"body":"<!-- ci-sweep:aaa -->","comments":[]}"#;
    let Some((out, calls)) = run("dedupe", A, ISSUES, view) else {
        return;
    };
    assert!(out.contains("Already reported."));
    assert!(calls.is_empty(), "{calls}");
}
