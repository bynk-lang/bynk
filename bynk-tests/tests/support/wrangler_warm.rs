//! One cold-cache provisioning of the `npx` wrangler per machine, however many
//! test processes start at once (1D.3d, #4043 Decision J).
//!
//! Every test that runs `npx -y <spec> …` resolves to the same
//! `~/.npm/_npx/<hash>` directory (the hash is of the package spec), and npm
//! does not lock it. Under nextest each test is its own process, so on a cold
//! cache several of them install into that directory at once and tear down
//! each other's half-extracted trees (`ENOTEMPTY`, then `TAR_ENTRY_ERROR
//! ENOENT`; see `bynk-tests/tests/wrangler/mod.rs`). CI's pre-warm step covers
//! CI; this covers a developer's first run, and any job that skips the step.
//!
//! [`warm`] takes an exclusive `File::lock` on one file in the temp directory,
//! and the first process to hold it runs `npx -y <spec> --version` while the
//! others wait. It then leaves a marker, so the processes that were waiting,
//! and every later one, see a provisioned cache and skip the install. A marker
//! older than [`MARKER_TTL`] is ignored, so a cache cleared since is warmed
//! again. Within one process a `Once` makes the check happen once.
//!
//! Shared by `bynk-tests` and by `bynk`'s own `bynk dev` tests (through a
//! `#[path]`, as `ports.rs` is), so both use the same lock and the same marker.

// Each test binary includes this whole module and uses part of it.
#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Once;
use std::time::Duration;

/// The wrangler package spec the tests provision through `npx`.
pub const SPEC: &str = bynk_driver::tools::pins::WRANGLER_SPEC;

/// How long a marker vouches for the cache. Long enough for a day's runs,
/// short enough that an `npm cache clean` is noticed by the next morning.
pub const MARKER_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// What [`warm_in`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Warmed {
    /// A fresh marker was there: another process provisioned the cache.
    Already,
    /// This call provisioned it, and left the marker.
    Now,
    /// The provisioning failed (no `npx`, no network), so no marker was left
    /// and the next process tries again. The test then fails or skips exactly
    /// as it would have without the warm-up.
    Failed,
    /// The lock file could not be opened or locked. Nothing was run.
    Unlocked,
}

/// Provision [`SPEC`] once for this machine before a test spawns it through
/// `npx`. Cheap after the first call in a process.
pub fn warm() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        warm_in(&std::env::temp_dir(), SPEC, || provision(SPEC));
    });
}

/// [`warm`] for a test that starts `bynk dev`, which runs an installed
/// wrangler when there is one and `npx` only otherwise.
pub fn warm_unless_installed() {
    let installed = Command::new("wrangler")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !installed {
        warm();
    }
}

/// The lock file and the marker for `spec` in `dir`.
pub fn paths(dir: &Path, spec: &str) -> (PathBuf, PathBuf) {
    let tag: String = spec
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    (
        dir.join(format!("bynk-wrangler-warm-{tag}.lock")),
        dir.join(format!("bynk-wrangler-warm-{tag}.done")),
    )
}

/// The cross-process step of [`warm`], with the lock's directory and the
/// provisioning as parameters so a test can run it against an empty
/// directory and a stand-in install.
pub fn warm_in(dir: &Path, spec: &str, provision: impl FnOnce() -> bool) -> Warmed {
    let (lock_path, marker) = paths(dir, spec);
    let Ok(lock) = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    else {
        return Warmed::Unlocked;
    };
    // Blocks while another process provisions; released when `lock` drops.
    if lock.lock().is_err() {
        return Warmed::Unlocked;
    }
    if is_fresh(&marker) {
        return Warmed::Already;
    }
    if !provision() {
        return Warmed::Failed;
    }
    // A marker that cannot be written costs only a repeat install later.
    let _ = std::fs::write(&marker, spec);
    Warmed::Now
}

fn is_fresh(marker: &Path) -> bool {
    std::fs::metadata(marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < MARKER_TTL)
}

/// `npx -y <spec> --version`, which installs `spec` into the npx cache when it
/// is not there. Routed through `cmd /C` on Windows so `npx.cmd` resolves.
fn provision(spec: &str) -> bool {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "npx"]);
        c
    } else {
        Command::new("npx")
    };
    cmd.args(["-y", spec, "--version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
