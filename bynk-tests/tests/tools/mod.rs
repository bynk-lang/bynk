//! Finding the external tools a test drives — `node`, `tsc`, `npx` — and
//! deciding whether a missing one skips the test or fails it.
//!
//! Shared by the first-party binding suites (standard-library S2, #1952). The
//! older suites (`tsc_verify.rs`, `fetch_headers.rs`, `kv_behaviour.rs`, the
//! workerd smokes) keep their own copies of these few helpers; they follow the
//! same rules.

#![allow(dead_code)]

use std::process::{Command, Stdio};

/// Is `name` on `PATH`? `where` is the Windows counterpart of `which`.
pub fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A `Command` for `program`, routed through `cmd /C` on Windows so npm's
/// `.cmd` shims (`tsc.cmd`, `npx.cmd`) resolve: Rust refuses to run a batch
/// script directly there (the BatBadBut hardening).
pub fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

/// How to run `tsc`: the one on `PATH`, else `npx` with the TypeScript major
/// the toolchain is tested against.
pub fn tsc_command() -> Option<Command> {
    if tool_exists("tsc") {
        return Some(base_command("tsc"));
    }
    if tool_exists("npx") {
        let mut c = base_command("npx");
        c.args(["--yes", "-p"])
            .arg(format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED))
            .arg("tsc");
        return Some(c);
    }
    None
}

/// Report a skipped test loudly, or fail it when the run requires the tool.
/// `required` comes from `require::is_required(<VAR>)` — the one implementation
/// of "empty reads as unset" (#965, #971), which this module must not copy.
/// Returns `true`, so a caller can write `if missing && skip(..) { return; }`.
pub fn skip(what: &str, required: bool, var: &str, reason: &str) -> bool {
    eprintln!("\n!!! {what} SKIPPED !!!\n{reason}\n");
    if required {
        panic!("{var} is set but {reason}");
    }
    true
}
