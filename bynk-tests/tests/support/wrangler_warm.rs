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
//! A failed install must not be retried by every waiting process in turn: that
//! would serialise N failures (no network, a stalled registry) where there was
//! one parallel one before. So the install is bounded by [`INSTALL_TIMEOUT`],
//! and a failure leaves a short-lived failure marker ([`FAILURE_TTL`]) that the
//! waiting processes read and give up on at once. They then fail or skip
//! exactly as they would have without the warm-up.
//!
//! The lock and the markers are named for the spec and for the npm cache they
//! vouch for (`npm_config_cache`, else `HOME`/`USERPROFILE`), so two users or
//! two jobs with different caches on one machine neither trust each other's
//! marker nor share a lock file one of them cannot open.
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

/// How long a failed install is believed: long enough for every process that
/// was waiting on it to give up at once, short enough that a fixed network is
/// noticed by the next run.
pub const FAILURE_TTL: Duration = Duration::from_secs(2 * 60);

/// The longest the install may take before it counts as failed: a cold
/// `wrangler@4` fetch takes well under a minute on a working network.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What [`warm_in`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Warmed {
    /// A fresh marker was there: another process provisioned the cache.
    Already,
    /// This call provisioned it, and left the marker.
    Now,
    /// The provisioning failed (no `npx`, no network, or it ran past
    /// [`INSTALL_TIMEOUT`]), or a process that held the lock just before saw
    /// it fail ([`FAILURE_TTL`]). No success marker was left, so a later run
    /// tries again. The test then fails or skips exactly as it would have
    /// without the warm-up.
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
/// wrangler when there is one and `npx` only otherwise. Checked once per
/// process.
pub fn warm_unless_installed() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Through `cmd /C` on Windows, so an installed `wrangler.cmd` resolves.
        let installed = shell("wrangler")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !installed {
            warm();
        }
    });
}

/// The lock file, the marker and the failure marker for `spec` in `dir`, for
/// this process's npm cache.
pub fn paths(dir: &Path, spec: &str) -> (PathBuf, PathBuf, PathBuf) {
    paths_for(dir, spec, &npm_cache_identity())
}

/// [`paths`] for an explicit cache identity.
pub fn paths_for(dir: &Path, spec: &str, cache: &str) -> (PathBuf, PathBuf, PathBuf) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cache.hash(&mut hasher);
    let mut tag: String = spec
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    tag.push_str(&format!("-{:016x}", hasher.finish()));
    (
        dir.join(format!("bynk-wrangler-warm-{tag}.lock")),
        dir.join(format!("bynk-wrangler-warm-{tag}.done")),
        dir.join(format!("bynk-wrangler-warm-{tag}.failed")),
    )
}

/// Which npm cache an install fills: `npm_config_cache` when it is set, else
/// the home directory whose `.npm` npm uses.
fn npm_cache_identity() -> String {
    [
        "npm_config_cache",
        "NPM_CONFIG_CACHE",
        "HOME",
        "USERPROFILE",
    ]
    .iter()
    .find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()))
    .unwrap_or_default()
}

/// The cross-process step of [`warm`], with the lock's directory and the
/// provisioning as parameters so a test can run it against an empty
/// directory and a stand-in install.
pub fn warm_in(dir: &Path, spec: &str, provision: impl FnOnce() -> bool) -> Warmed {
    let (lock_path, marker, failed) = paths(dir, spec);
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
    if is_fresh(&marker, MARKER_TTL) {
        return Warmed::Already;
    }
    // Another process failed just now: waiting processes give up at once
    // rather than each repeating a failure that takes minutes.
    if is_fresh(&failed, FAILURE_TTL) {
        return Warmed::Failed;
    }
    if !provision() {
        let _ = std::fs::write(&failed, spec);
        return Warmed::Failed;
    }
    // A marker that cannot be written costs only a repeat install later.
    let _ = std::fs::remove_file(&failed);
    let _ = std::fs::write(&marker, spec);
    Warmed::Now
}

fn is_fresh(marker: &Path, ttl: Duration) -> bool {
    std::fs::metadata(marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < ttl)
}

/// `program`, routed through `cmd /C` on Windows so an npm `.cmd` shim
/// resolves (Rust will not run a batch script directly there).
fn shell(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", program]);
        c
    } else {
        Command::new(program)
    }
}

/// `npx -y <spec> --version`, which installs `spec` into the npx cache when it
/// is not there, given at most [`INSTALL_TIMEOUT`]: one past it is killed and
/// counts as a failure.
fn provision(spec: &str) -> bool {
    let Ok(mut child) = shell("npx")
        .args(["-y", spec, "--version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + INSTALL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
