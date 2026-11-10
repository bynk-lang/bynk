//! Shared by the tests that spawn `bynk dev` (#3233).
//!
//! `bynk dev` can exit before it is ready (a start-up race, a port another
//! process took). The tests used to send its output to `/dev/null`, so the only
//! evidence was an exit status. Writing it to a log file and quoting the tail in
//! the failure puts the cause in the CI log.

use std::fs::File;
use std::path::Path;
use std::process::{ExitStatus, Stdio};

/// Both of `bynk dev`'s output streams, appended to the file at `log`.
pub fn log_to(log: &Path) -> (Stdio, Stdio) {
    let file = File::create(log).expect("create the bynk dev log");
    let err = file.try_clone().expect("clone the bynk dev log handle");
    (Stdio::from(file), Stdio::from(err))
}

/// The failure message for a `bynk dev` that exited before it was ready: the
/// status, then the last lines of what it wrote.
pub fn exited_early(status: ExitStatus, log: &Path) -> String {
    const TAIL_LINES: usize = 60;
    let text = std::fs::read_to_string(log)
        .unwrap_or_else(|e| format!("(could not read {}: {e})", log.display()));
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    format!(
        "bynk dev exited before it was ready: {status}\n--- its output (last {TAIL_LINES} lines) ---\n{tail}"
    )
}
