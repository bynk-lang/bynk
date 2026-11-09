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

/// Runs the report with `findings` on stdin. `open_issue` is the number the stub
/// `gh issue list` returns (empty = none) and `known` the text `gh issue view`
/// returns. Returns (stdout, the stub's call log).
fn run(tag: &str, findings: &str, open_issue: &str, known: &str) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("bynk-sweep-report-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("known.txt"), known).unwrap();
    let gh = dir.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/sh
case "$1 $2" in
  "issue list") printf '%s' "$STUB_OPEN_ISSUE" ;;
  "issue view") cat "$STUB_DIR/known.txt" ;;
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
        .env("STUB_OPEN_ISSUE", open_issue)
        .env("GH_REPO", "o/r")
        .env("GITHUB_SERVER_URL", "https://github.test")
        .env("GITHUB_RUN_ID", "42")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(findings.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "script failed");
    let calls = std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    (String::from_utf8_lossy(&out.stdout).into_owned(), calls)
}

const A: &str = "- PR #1: head `aaa` has no `ci.yml` run. <!-- ci-sweep:aaa -->\n";
const B: &str = "- PR #2: head `bbb` has no `ci.yml` run. <!-- ci-sweep:bbb -->\n";

#[test]
fn no_findings_writes_nothing() {
    let (out, calls) = run("none", "", "", "");
    assert!(out.contains("Every open PR head"));
    assert!(calls.is_empty());
}

#[test]
fn no_open_issue_creates_one_with_every_finding() {
    let (_, calls) = run("create", &format!("{A}{B}"), "", "");
    assert!(
        calls.starts_with("issue create --title CI runs missing"),
        "{calls}"
    );
    assert!(calls.contains("ci-sweep:aaa") && calls.contains("ci-sweep:bbb"));
    assert!(calls.contains("https://github.test/o/r/actions/runs/42"));
}

#[test]
fn an_open_issue_gets_a_comment_with_only_the_new_shas() {
    let (_, calls) = run(
        "comment",
        &format!("{A}{B}"),
        "7",
        "body <!-- ci-sweep:aaa -->",
    );
    assert!(calls.starts_with("issue comment 7 --body"), "{calls}");
    assert!(calls.contains("ci-sweep:bbb"), "{calls}");
    assert!(
        !calls.contains("ci-sweep:aaa"),
        "a known SHA is not repeated: {calls}"
    );
}

#[test]
fn already_reported_shas_write_nothing() {
    let (out, calls) = run("dedupe", A, "7", "x\n<!-- ci-sweep:aaa -->\n");
    assert!(out.contains("Already reported."));
    assert!(calls.is_empty(), "{calls}");
}
