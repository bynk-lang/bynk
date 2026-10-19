//! Shelling the resolved `bynkc` — shared by the commands that delegate to it.
//!
//! `bynk test` always delegates (it orchestrates external `tsc`/`node` anyway,
//! and delegating through the driver's resolution is the #487/#486 win); `bynk
//! check`/`fmt` delegate only under a `BYNK_BYNKC` override, so a
//! developer-pinned compiler still governs the result. All three inherit stdio
//! and propagate the child's exit status through [`exit_status_byte`].

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, ExitCode};

/// Map a child's [`std::process::ExitStatus`] to a process exit byte. A normal exit
/// propagates the code. Signal death is *not* uniformly a clean stop: a
/// shared Ctrl-C (SIGINT) is — the terminal delivered it to us too — but a
/// SIGSEGV or the OOM killer's SIGKILL is a real failure, and mapping it to
/// success made a crashed \`bynkc test\` read as passing in CI. Non-SIGINT
/// signals exit \`128 + signal\`, the shell convention.
pub fn exit_status_byte(status: &std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return code.clamp(0, 255) as u8;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            // SIGINT is 2 on every Unix.
            if sig == 2 {
                return 0;
            }
            return 128u8.saturating_add(sig.clamp(0, 127) as u8);
        }
    }
    1
}

/// Shell `bynkc <args>` at `bynkc`, inheriting stdio, and return its exit code.
/// A spawn failure (a missing or unexecutable binary) is surfaced as a driver
/// error naming the path, so a bad `BYNK_BYNKC` override is diagnosable.
pub fn delegate<I, S>(bynkc: &Path, args: I) -> ExitCode
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    delegate_with_env(bynkc, args, &[])
}

/// [`delegate`], with `env` set for `bynkc`.
pub fn delegate_with_env<I, S>(bynkc: &Path, args: I, env: &[(&str, &OsStr)]) -> ExitCode
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(bynkc);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match cmd.status() {
        Ok(s) => ExitCode::from(exit_status_byte(&s)),
        Err(e) => {
            eprintln!("bynk: could not run bynkc ({}): {e}", bynkc.display());
            ExitCode::FAILURE
        }
    }
}
