//! The shared detection probe: **presence + version + provenance**.
//!
//! This generalises `bynkc`'s old `tool_exists` (which shelled the POSIX
//! `which` and conceded it was Unix-only). Detection is backed by the `which`
//! crate, which handles `PATHEXT`/`where` on Windows, so a *user-facing*
//! environment check tells the truth on every platform (v0.46 portability
//! decision).
//!
//! Lookups go through a [`Toolbox`] so the capability/exit matrix and the
//! output goldens can run against a deterministic fake instead of the host.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::npm_install::PackageManager;

/// A parsed tool version. Missing components default to zero, so `"v18"` parses
/// as `18.0.0` and compares sensibly against a major-version floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Extract the first dotted-decimal run from arbitrary `--version` output
    /// (e.g. `"v18.17.0"`, `"Version 5.4.2"`, `"⛅️ wrangler 3.90.0"`).
    pub fn parse(s: &str) -> Option<Version> {
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() && !b[i].is_ascii_digit() {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        let mut nums = [0u32; 3];
        let mut slot = 0;
        while i < b.len() && slot < 3 {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            nums[slot] = s[start..i].parse().ok()?;
            slot += 1;
            if i < b.len() && b[i] == b'.' {
                i += 1;
            } else {
                break;
            }
        }
        Some(Version {
            major: nums[0],
            minor: nums[1],
            patch: nums[2],
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Where a tool was found — the **provenance** half of a probe. The distinction
/// between [`Provenance::Path`]/[`Provenance::ProjectLocal`] (installed) and
/// [`Provenance::Runner`] (fetchable on demand) is the whole point: `npx --yes`
/// (or `pnpm dlx`, `bunx`) will *download* a package at first use, so it must never read as a green
/// "ok".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// On the global `PATH`, at this resolved path.
    Path(PathBuf),
    /// In a project-local `node_modules/.bin` (preferred over `PATH`).
    ProjectLocal(PathBuf),
    /// Not installed, but **provisionable** through the package manager's
    /// runner (`npx --yes`, `pnpm dlx`, `bunx`) — a deferred download, not a
    /// present tool. Holds the manager whose runner it is.
    Runner(PackageManager),
    /// Not found, and not provisionable.
    Missing,
}

impl Provenance {
    /// The stable token used in `--format json`/`short` and the human table.
    pub fn token(&self) -> &'static str {
        match self {
            Provenance::Path(_) => "path",
            Provenance::ProjectLocal(_) => "project-local",
            Provenance::Runner(PackageManager::Pnpm) => "pnpm-dlx",
            Provenance::Runner(PackageManager::Bun) => "bunx",
            // Yarn classic has no runner of its own and never reaches here.
            Provenance::Runner(PackageManager::Npm | PackageManager::Yarn) => "npx",
            Provenance::Missing => "missing",
        }
    }

    /// The runner that would fetch the tool (`npx`, `pnpm dlx`, `bunx`), for
    /// provisionable tools.
    pub fn runner_label(&self) -> Option<&'static str> {
        match self {
            Provenance::Runner(m) => m.runner_label().or(Some("npx")),
            _ => None,
        }
    }

    /// The resolved path, if the tool is actually installed.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Provenance::Path(p) | Provenance::ProjectLocal(p) => Some(p),
            _ => None,
        }
    }
}

/// One detection result: a tool, its version (if installed and it reports one),
/// and where it came from.
#[derive(Debug, Clone)]
pub struct Probe {
    pub tool: String,
    pub version: Option<Version>,
    pub provenance: Provenance,
}

impl Probe {
    /// Installed on disk (`PATH` or project-local) — not merely provisionable.
    pub fn is_present(&self) -> bool {
        matches!(
            self.provenance,
            Provenance::Path(_) | Provenance::ProjectLocal(_)
        )
    }

    /// Absent but fetchable on demand through a runner (`npx`, `pnpm dlx`, `bunx`).
    pub fn is_provisionable(&self) -> bool {
        matches!(self.provenance, Provenance::Runner(_))
    }

    /// Neither installed nor provisionable.
    pub fn is_missing(&self) -> bool {
        matches!(self.provenance, Provenance::Missing)
    }
}

/// How a detection should look: where to search, and whether a runner's
/// fetch-on-demand counts as a (provisionable) fallback. `node` itself is never
/// `allow_npx` — you cannot `npx` a runtime. `manager` is the package manager
/// whose runner is tried first (`None`: npm, so `npx`).
#[derive(Debug, Clone, Copy, Default)]
pub struct DetectOpts<'a> {
    pub project_root: Option<&'a Path>,
    pub allow_npx: bool,
    pub manager: Option<PackageManager>,
}

/// Abstraction over the host so detection is testable. The real implementation
/// is [`SystemToolbox`]; tests supply a deterministic fake.
pub trait Toolbox {
    /// Resolve `tool` on the global `PATH` (with `PATHEXT` semantics).
    fn on_path(&self, tool: &str) -> Option<PathBuf>;
    /// Resolve `tool` inside a specific directory (a `node_modules/.bin`).
    fn in_dir(&self, dir: &Path, tool: &str) -> Option<PathBuf>;
    /// Run `<path> --version` and parse the first version it prints.
    fn version(&self, path: &Path) -> Option<Version>;
    /// Is `npx` itself available to provision packages on demand?
    fn npx_available(&self) -> bool;
    /// #3477: does the Docker CLI at `path` reach a running daemon (`docker
    /// info` succeeds)? Building a container image needs one. The default says
    /// yes, so a fake that does not model Docker is not made to fail.
    fn docker_daemon_running(&self, _path: &Path) -> bool {
        true
    }
}

/// Detect a single tool: project-local first (it wins over a global install),
/// then `PATH`, then — only if `allow_npx` — a runner-provisionable fallback,
/// else missing.
pub fn detect(tb: &dyn Toolbox, tool: &str, opts: DetectOpts<'_>) -> Probe {
    if let Some(root) = opts.project_root {
        let bin = root.join("node_modules").join(".bin");
        if let Some(p) = tb.in_dir(&bin, tool) {
            let version = tb.version(&p);
            return Probe {
                tool: tool.to_string(),
                version,
                provenance: Provenance::ProjectLocal(p),
            };
        }
    }
    if let Some(p) = tb.on_path(tool) {
        let version = tb.version(&p);
        return Probe {
            tool: tool.to_string(),
            version,
            provenance: Provenance::Path(p),
        };
    }
    if opts.allow_npx
        && let Some(runner) = opts.manager.unwrap_or(PackageManager::Npm).runner(tb)
    {
        return Probe {
            tool: tool.to_string(),
            version: None,
            provenance: Provenance::Runner(runner),
        };
    }
    Probe {
        tool: tool.to_string(),
        version: None,
        provenance: Provenance::Missing,
    }
}

/// Detect `tool` for the project at `project_root`: project-local first, then
/// `PATH`, then the runner of the package manager detected for the project.
/// Returns that manager too, since hints follow it.
pub fn detect_in_project(
    tb: &dyn Toolbox,
    tool: &str,
    project_root: &Path,
) -> (PackageManager, Probe) {
    let manager = crate::npm_install::manager_in(project_root, tb);
    let opts = DetectOpts {
        project_root: Some(project_root),
        allow_npx: true,
        manager: Some(manager),
    };
    (manager, detect(tb, tool, opts))
}

/// #1758: the program to spawn for `tool`: the path `which` resolves on
/// `PATH`, or the bare name when it resolves nothing. Spawn tools through this
/// rather than `Command::new("<name>")`. On Windows, `Command` looks up a bare
/// name with `.exe` only, so an npm shim (`npx.cmd`, `tsc.cmd`) that detection
/// finds through `PATHEXT` is never started. An explicit path to a `.cmd` is
/// spawned correctly, with its arguments escaped.
///
/// Resolved through [`SystemToolbox::on_path`], the same lookup detection uses,
/// so the spawned program is the one a probe of `tool` reports.
pub fn program_path(tool: &str) -> PathBuf {
    SystemToolbox
        .on_path(tool)
        .unwrap_or_else(|| PathBuf::from(tool))
}

/// The real host: `which`-crate lookups and a `--version` shell-out.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemToolbox;

impl Toolbox for SystemToolbox {
    fn on_path(&self, tool: &str) -> Option<PathBuf> {
        which::which(tool).ok()
    }

    fn in_dir(&self, dir: &Path, tool: &str) -> Option<PathBuf> {
        // `which_in` applies the same PATHEXT/extension resolution inside the
        // given directory, so a Windows `tsc.cmd`/`tsc.exe` resolves too.
        which::which_in(tool, Some(dir.as_os_str()), dir).ok()
    }

    fn version(&self, path: &Path) -> Option<Version> {
        let out = Command::new(path).arg("--version").output().ok()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let parsed = Version::parse(&stdout);
        if parsed.is_some() {
            return parsed;
        }
        // Some tools print their version banner to stderr.
        Version::parse(&String::from_utf8_lossy(&out.stderr))
    }

    fn npx_available(&self) -> bool {
        which::which(OsStr::new("npx")).is_ok()
    }

    fn docker_daemon_running(&self, path: &Path) -> bool {
        Command::new(path)
            .arg("info")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_version_banners() {
        assert_eq!(
            Version::parse("v18.17.0"),
            Some(Version {
                major: 18,
                minor: 17,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("Version 5.4.2"),
            Some(Version {
                major: 5,
                minor: 4,
                patch: 2
            })
        );
        assert_eq!(
            Version::parse("⛅️ wrangler 3.90.0"),
            Some(Version {
                major: 3,
                minor: 90,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("v20"),
            Some(Version {
                major: 20,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(Version::parse("no digits here"), None);
    }
}
