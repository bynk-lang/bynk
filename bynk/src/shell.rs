//! Child-process exit statuses — shared by `dev` and `deploy`, which run
//! `wrangler` (#2998 removed the shelled `bynkc` they once shared with).

/// Map a child's [`std::process::ExitStatus`] to a process exit byte. A normal exit
/// propagates the code. Signal death is *not* uniformly a clean stop: a
/// shared Ctrl-C (SIGINT) is — the terminal delivered it to us too — but a
/// SIGSEGV or the OOM killer's SIGKILL is a real failure, and mapping it to
/// success made a crashed child read as passing in CI. Non-SIGINT
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
