//! C4c (#2940, Decision B): `scripts/ci/run-unless-sibling-fails.sh` stops its
//! command when the sibling job has failed, and otherwise passes the command's
//! own exit status through. A stub `node` stands in for the jobs API.
// Linux only: the script uses util-linux `setsid` and only ever runs on the Linux
// `test-system` job (macOS has no `setsid`).
#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci/run-unless-sibling-fails.sh")
}

/// Runs the watcher around `cmd` with a `node` that answers `sibling_failed`.
fn run(sibling_failed: bool, cmd: &[&str]) -> (i32, String, Duration) {
    run_with_stub(&format!("echo {sibling_failed}"), cmd)
}

/// Like [`run`], with the stub `node`'s body given.
fn run_with_stub(stub_body: &str, cmd: &[&str]) -> (i32, String, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "bynk-sibling-watch-{}-{:x}-{}",
        std::process::id(),
        stub_body
            .bytes()
            .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)),
        cmd.join("_").replace('/', "")
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // A stub `node` that answers like the jobs lookup would.
    let node = dir.join("node");
    std::fs::write(&node, format!("#!/bin/sh\n{stub_body}\n")).unwrap();
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let start = Instant::now();
    let out = Command::new("bash")
        .arg(script())
        .arg("Test suite (ubuntu-latest)")
        .args(cmd)
        .env("PATH", path)
        .env("POLL_SECONDS", "1")
        .env("GITHUB_REPOSITORY", "o/r")
        .env("GITHUB_RUN_ID", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        start.elapsed(),
    )
}

#[test]
fn a_failed_sibling_stops_the_command_and_fails_with_a_message() {
    let (code, stdout, took) = run(true, &["sleep", "60"]);
    assert_eq!(code, 1);
    assert!(
        stdout.contains("'Test suite (ubuntu-latest)' job failed"),
        "{stdout}"
    );
    assert!(took < Duration::from_secs(30), "took {took:?}");
}

#[test]
fn a_healthy_sibling_passes_the_commands_status_through() {
    assert_eq!(run(false, &["true"]).0, 0);
    assert_eq!(run(false, &["sh", "-c", "exit 3"]).0, 3);
}

/// The safety property: a lookup that errors or prints garbage never stops the
/// command or changes its status (the watcher only ever stops a command early
/// on a positive answer).
#[test]
fn a_failing_or_garbled_lookup_leaves_the_commands_status_alone() {
    let cmd = ["sh", "-c", "sleep 3; exit 4"];
    assert_eq!(run_with_stub("echo boom >&2; exit 1", &cmd).0, 4);
    assert_eq!(run_with_stub("echo not-json", &cmd).0, 4);
    assert_eq!(run_with_stub("", &cmd).0, 4);
}
