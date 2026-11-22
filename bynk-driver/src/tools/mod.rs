//! Back-half paydown 1D.5b (#4003, Decision F): the spawning of the external
//! tools the driver runs, in one place: `tsc`, `tsx`, `node`, `git`, and the
//! npm, pnpm, bun and yarn installs.
//!
//! - [`pins`]: the versions fetched on demand (TypeScript, `tsx`).
//! - [`ladder`]: the `tsc` and `tsx` runner ladders.
//! - Here: the `node` and `git` commands, and [`run_to_completion`], which runs
//!   an install. What to run for each package manager (`PackageManager`'s
//!   runner and install methods) stays in [`crate::npm_install`].
//!
//! Each tool is found exactly as its callers always found it, so moving a
//! spawn here changes no error text and no edge case: `node` through
//! [`crate::probe::program_path`] for a test run ([`node_command`]) and through
//! detection for `bynk test --inspect` ([`resolved_node_command`]), `git` by
//! its bare name. Detection itself stays in [`crate::probe`], which wrangler
//! and Docker also use. Wrangler is not spawned here: 1D.3's `Wrangler` client
//! takes it.

pub mod ladder;
pub mod pins;

pub use ladder::{Ladder, Rung};

use std::path::Path;
use std::process::Command;

/// `node`, found as [`crate::probe::program_path`] finds it: its resolved
/// path, else the bare name.
pub fn node_command() -> Command {
    Command::new(crate::probe::program_path("node"))
}

/// `node` at the path detection resolves, or `None` when it is not on PATH.
pub fn resolved_node_command() -> Option<Command> {
    ladder::resolve_tool("node").map(Command::new)
}

/// `git`, by its bare name. A caller adds its own policy: `bynk fetch` hardens
/// every invocation (`fetch::git`).
pub fn git_command() -> Command {
    Command::new("git")
}

/// Run `program args` in `dir` to completion. `Ok((success, output))`, where the
/// output is the tool's stderr, or its stdout when stderr is blank; `Err` when
/// the tool could not be started.
pub fn run_to_completion(
    program: &Path,
    args: &[&str],
    dir: &Path,
) -> Result<(bool, String), String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run `{}`: {e}", program.display()))?;
    let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&out.stdout).into_owned();
    }
    Ok((out.status.success(), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_is_found_by_its_bare_name() {
        assert_eq!(git_command().get_program(), std::ffi::OsStr::new("git"));
        assert_eq!(git_command().get_args().count(), 0);
    }

    #[test]
    fn node_is_found_as_program_path_finds_it() {
        assert_eq!(
            node_command().get_program(),
            crate::probe::program_path("node").as_os_str()
        );
    }

    #[test]
    fn a_tool_that_cannot_start_is_an_error_naming_it() {
        let missing = Path::new("/no/such/bynk-tool");
        let err = run_to_completion(missing, &[], Path::new(".")).unwrap_err();
        assert!(err.starts_with("could not run `"), "{err}");
        assert!(err.contains("bynk-tool"), "{err}");
    }
}
