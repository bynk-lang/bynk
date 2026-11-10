//! Shared by the tests that spawn `bynk dev` (#3233).
//!
//! `bynk dev` can exit before it is ready (a start-up race, a port another
//! process took). The tests used to send its output to `/dev/null`, so the only
//! evidence was an exit status. Writing it to a log file and quoting the tail in
//! the failure puts the cause in the CI log.

// Each test binary includes this whole module and uses part of it.
#![allow(dead_code)]

use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

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

/// Whether `tool --version` runs.
pub fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Stops `bynk dev`'s whole process group on drop: `SIGTERM`, then `SIGKILL`
/// for anything still there. The group is the one the test created.
#[cfg(unix)]
pub struct Group(pub Child);

#[cfg(unix)]
impl Drop for Group {
    fn drop(&mut self) {
        let pid = libc::pid_t::try_from(self.0.id()).expect("a pid fits pid_t");
        // SAFETY: `kill` takes plain integers; a negative pid names the
        // process group this test created for `bynk dev`.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let _ = self.0.try_wait();
            // SAFETY: as above; signal 0 only checks the group still exists.
            if unsafe { libc::kill(-pid, 0) } != 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // SAFETY: as above.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = self.0.wait();
    }
}
