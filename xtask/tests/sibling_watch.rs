//! C4c (#2940, Decision B): `scripts/ci/run-unless-sibling-fails.sh` stops its
//! command when the sibling job has failed, and otherwise passes the command's
//! own exit status through. A stub `node` stands in for the jobs API.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci/run-unless-sibling-fails.sh")
}

/// Runs the watcher around `cmd` with a `node` that answers `sibling_failed`.
fn run(sibling_failed: bool, cmd: &[&str]) -> (i32, String, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "bynk-sibling-watch-{}-{sibling_failed}-{}",
        std::process::id(),
        cmd.join("_").replace('/', "")
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // A stub `node` that answers like the jobs lookup would.
    let node = dir.join("node");
    std::fs::write(&node, format!("#!/bin/sh\necho {sibling_failed}\n")).unwrap();
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
