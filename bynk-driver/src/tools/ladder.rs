//! The runner ladders for `tsc` and `tsx`: the tool itself when it is on PATH,
//! then the package manager's runner (`npx`, `pnpm dlx`, `bunx`) fetching it.
//!
//! `bynk test` and `bynk check --emit` both walk the `tsc` ladder, so it is
//! written once here. A rung is resolved through detection
//! ([`crate::probe::detect`]) when the walk reaches it, and spawned at the path
//! detection found, so a Windows npm shim (`tsc.cmd`) is also the program that
//! runs (#1758).

use crate::npm_install::PackageManager;
use crate::probe::{DetectOpts, SystemToolbox, Toolbox};
use crate::tools::pins;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The programs to try, in order, to run `tool` (a binary of `package`, which
/// may be pinned): the tool itself, then the runner that fetches it on demand
/// when `manager` (or `npx`, for a manager with no runner) is on PATH. Each is
/// a program name with its leading arguments.
pub fn tool_ladder(
    tool: &str,
    package: &str,
    manager: PackageManager,
    tb: &dyn Toolbox,
) -> Vec<(String, Vec<String>)> {
    let mut rungs = vec![(tool.to_string(), Vec::new())];
    if let Some(runner) = manager.runner(tb)
        && let Some(program) = runner.runner_program()
    {
        rungs.push((program.to_string(), runner.runner_args(package, tool)));
    }
    rungs
}

/// [`tool_ladder`] over the real PATH.
pub fn system_tool_ladder(
    tool: &str,
    package: &str,
    manager: PackageManager,
) -> Vec<(String, Vec<String>)> {
    tool_ladder(tool, package, manager, &SystemToolbox)
}

/// Where `name` is installed on `PATH`, if it is. #1758: callers spawn this
/// path, not the bare name, so a Windows npm shim (`tsc.cmd`) that detection
/// finds is also the program that runs.
pub(crate) fn resolve_tool(name: &str) -> Option<PathBuf> {
    crate::probe::detect(&SystemToolbox, name, DetectOpts::default())
        .provenance
        .path()
        .map(Path::to_path_buf)
}

/// #1758: a runner that was found but would not start. Recorded instead of
/// silently skipped, so a run with no working runner reports the real cause
/// rather than advising an install of something already installed.
pub(crate) fn start_failure(path: &Path, e: &std::io::Error) -> String {
    format!(
        "`{}` was found but could not be started: {e}",
        path.display()
    )
}

/// A tool's ladder over the real PATH, for one package manager.
pub struct Ladder {
    rungs: Vec<(String, Vec<String>)>,
}

impl Ladder {
    /// `tsc`, then the runner fetching [`pins::typescript_spec`].
    pub fn tsc(manager: PackageManager) -> Self {
        Ladder {
            rungs: system_tool_ladder("tsc", &pins::typescript_spec(), manager),
        }
    }

    /// `tsx`, then the runner fetching [`pins::TSX_SPEC`].
    pub fn tsx(manager: PackageManager) -> Self {
        Ladder {
            rungs: system_tool_ladder("tsx", pins::TSX_SPEC, manager),
        }
    }

    /// Each rung that is on PATH, in order. A rung is resolved only when the
    /// walk reaches it, so a caller that stops at the first that runs never
    /// probes the rest.
    pub fn rungs(&self) -> impl Iterator<Item = Rung> + '_ {
        self.rungs.iter().filter_map(|(program, prefix)| {
            resolve_tool(program).map(|path| Rung {
                path,
                prefix: prefix.clone(),
            })
        })
    }
}

/// One resolved rung of a [`Ladder`].
pub struct Rung {
    path: PathBuf,
    prefix: Vec<String>,
}

impl Rung {
    /// The rung's command: its resolved program and leading arguments. The
    /// caller adds its own.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.path);
        cmd.args(&self.prefix);
        cmd
    }

    /// Why this rung, found on PATH, did not start ([`start_failure`]).
    pub fn start_failure(&self, e: &std::io::Error) -> String {
        start_failure(&self.path, e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Version;

    /// A toolbox with chosen programs on PATH and `npx` present or not.
    struct Tools {
        path: &'static [&'static str],
        npx: bool,
    }

    impl Toolbox for Tools {
        fn on_path(&self, tool: &str) -> Option<PathBuf> {
            self.path
                .contains(&tool)
                .then(|| PathBuf::from(format!("/bin/{tool}")))
        }
        fn in_dir(&self, _: &Path, _: &str) -> Option<PathBuf> {
            None
        }
        fn version(&self, _: &Path) -> Option<Version> {
            None
        }
        fn npx_available(&self) -> bool {
            self.npx
        }
    }

    #[test]
    fn the_tool_ladder_is_the_tool_then_the_runner() {
        let tb = Tools {
            path: &["pnpm"],
            npx: true,
        };
        let typescript = pins::typescript_spec();
        let rungs = tool_ladder("tsc", &typescript, PackageManager::Pnpm, &tb);
        assert_eq!(
            rungs,
            [
                ("tsc".to_string(), vec![]),
                (
                    "pnpm".to_string(),
                    ["dlx", "--package", typescript.as_str(), "tsc"]
                        .map(String::from)
                        .to_vec()
                )
            ]
        );
        // Yarn classic goes through `npx`, as before.
        let yarn = tool_ladder("tsx", pins::TSX_SPEC, PackageManager::Yarn, &tb);
        assert_eq!(yarn[1].0, "npx");
        assert_eq!(yarn[1].1, ["--yes", "-p", "tsx", "tsx"]);
        // With no runner at all, only the tool itself is tried.
        let bare = Tools {
            path: &[],
            npx: false,
        };
        assert_eq!(
            tool_ladder("tsx", pins::TSX_SPEC, PackageManager::Yarn, &bare).len(),
            1
        );
    }

    #[test]
    fn each_manager_puts_its_own_runner_on_the_second_rung() {
        let tb = Tools {
            path: &["pnpm", "bunx"],
            npx: true,
        };
        let typescript = pins::typescript_spec();
        let second = |m: PackageManager| tool_ladder("tsc", &typescript, m, &tb)[1].clone();
        assert_eq!(
            second(PackageManager::Npm),
            (
                "npx".to_string(),
                ["--yes", "-p", typescript.as_str(), "tsc"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(second(PackageManager::Pnpm).0, "pnpm");
        assert_eq!(
            second(PackageManager::Bun),
            (
                "bunx".to_string(),
                ["--package", typescript.as_str(), "tsc"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(second(PackageManager::Yarn).0, "npx");
    }

    #[test]
    fn a_rung_runs_its_path_with_its_prefix() {
        let rung = Rung {
            path: PathBuf::from("/bin/npx"),
            prefix: ["--yes", "-p", "tsx", "tsx"].map(String::from).to_vec(),
        };
        let cmd = rung.command();
        assert_eq!(cmd.get_program(), std::ffi::OsStr::new("/bin/npx"));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args, ["--yes", "-p", "tsx", "tsx"]);
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(
            rung.start_failure(&e)
                .starts_with("`/bin/npx` was found but could not be started: "),
            "{}",
            rung.start_failure(&e)
        );
    }
}
