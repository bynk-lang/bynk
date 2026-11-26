//! Shared by the tests that spawn `bynk dev` (#3233).
//!
//! `bynk dev` can exit before it is ready (a start-up race, a port another
//! process took). The tests used to send its output to `/dev/null`, so the only
//! evidence was an exit status. Writing it to a log file and quoting the tail in
//! the failure puts the cause in the CI log.

// Each test binary includes this whole module and uses part of it.
#![allow(dead_code)]

/// The one cold-cache provisioning of the `npx` wrangler per machine, shared
/// with `bynk-tests`'s smokes so both take the same lock (1D.3d, #4043).
#[path = "../../../bynk-tests/tests/support/wrangler_warm.rs"]
pub mod wrangler_warm;

use std::ffi::OsStr;
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

/// Provision the `npx` wrangler for this machine before starting `bynk dev`,
/// unless an installed one is what `bynk dev` will run. Without it, parallel
/// test processes on a cold npx cache install into one directory at once and
/// break each other's install.
pub fn warm_wrangler() {
    wrangler_warm::warm_unless_installed();
}

/// #4555: `cmd` with Wrangler's usage telemetry off. The constant is the one `bynk-tests`' harness
/// uses ([`wrangler_warm::TELEMETRY_OFF`], from the shared `wrangler_warm.rs`), so a test starts no
/// Wrangler that tries to reach Cloudflare's telemetry hosts, wherever it runs. A child inherits it.
pub fn telemetry_off(cmd: &mut Command) -> &mut Command {
    cmd.env(
        wrangler_warm::TELEMETRY_OFF.0,
        wrangler_warm::TELEMETRY_OFF.1,
    )
}

/// A command for `program`, a `wrangler`, an `npx`, or a probe of one (`program --version`), with
/// Wrangler's telemetry off ([`telemetry_off`]). Every test that starts one builds it here.
pub fn tool_command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    telemetry_off(&mut cmd);
    cmd
}

/// The `bynk` binary under test, with Wrangler's telemetry off ([`telemetry_off`]). `bynk dev` and
/// `bynk test --against dev` start a Wrangler as a child, which inherits it. Every test that runs
/// `bynk` where it may start one builds it here.
pub fn bynk_command() -> Command {
    tool_command(env!("CARGO_BIN_EXE_bynk"))
}

/// Whether `tool --version` runs.
pub fn have(tool: &str) -> bool {
    tool_command(tool)
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
