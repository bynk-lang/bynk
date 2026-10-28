//! Reading and atomically writing `bynk.schema.lock` — the driver's side of
//! `CompileOptions::schema_registry` (#1078, R2.3/T0.7 residue).
//!
//! Ported verbatim from what used to be `bynk-emit/src/project/schema_registry.rs`'s
//! `read`/`write` — same crash-safety discipline (temp file + `sync_all` +
//! rename + best-effort directory fsync), same unchanged-content no-op. Only
//! the location moved: `bynk-emit` now only ever sees pre-read content (via
//! `SchemaLock::On`) and hands back serialized content to write (via
//! `ProjectOutput::schema_lock`) — it touches no disk for this file, same as
//! it no longer does for `.bynk` sources on the real CLI path (#1077/#1081).
//!
//! `bynk-emit`'s own `schema_registry::parse`/`serialize` are the pure
//! counterparts this module's [`read`]/[`write()`] wrap disk I/O around.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const LOCK_FILE: &str = "bynk.schema.lock";

/// Where the registry lives for a given project root — exposed so callers
/// reporting a write failure can name the actual file, not just the root.
pub fn lock_path(project_root: &Path) -> PathBuf {
    project_root.join(LOCK_FILE)
}

/// Read `bynk.schema.lock`'s raw content from `project_root`, if it exists.
/// `Ok(None)` means **verified absent** — no such file — which is the only
/// thing `CompileOptions`' `SchemaLock::On { existing: None }` is allowed to
/// mean (see that type's own doc). A present-but-unreadable file is a real
/// `io::Error`, not folded into `None` — silently treating a permission
/// error, a non-traversable ancestor, or any other stat/read failure as
/// "fresh project" would risk re-baselining a real registry's history
/// exactly as wrongly as never reading it at all.
///
/// #1085 review: a prior version checked `path.exists()` first and only then
/// read — `exists()` maps *every* metadata error to `false`, not just
/// "not found," so a non-`ENOENT` stat failure (or a dangling symlink, or a
/// race with another process between the two calls) would have silently
/// produced `Ok(None)` here. Reading directly and matching on
/// `ErrorKind::NotFound` is both narrower (only the one error kind degrades)
/// and race-free (one syscall, not two).
pub fn read(project_root: &Path) -> io::Result<Option<String>> {
    read_named(project_root, LOCK_FILE)
}

/// [`read`] for any lock file at the project root: the contracts lock
/// (#2554) is read with the same verified-absent discipline.
pub(crate) fn read_named(project_root: &Path, file: &str) -> io::Result<Option<String>> {
    match std::fs::read_to_string(project_root.join(file)) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The committed lock as a **read-only** [`SchemaLock::On`] — how `check` and
/// `test` reconcile events against `bynk.schema.lock` (#2078). Nothing is
/// written back: the caller drops `ProjectOutput::schema_lock` (and a
/// `ProjectCheck` carries none), so a passing additive change leaves the lock
/// byte-identical for `bynkc compile`, `bynk dev` or `bynk deploy` to record.
/// A missing lock is a fresh project, which baselines silently, exactly as a
/// build treats it. A read failure is a message naming the file, for the
/// caller to report.
///
/// [`SchemaLock::On`]: bynk_emit::project::SchemaLock::On
pub fn read_only(project_root: &Path) -> Result<bynk_emit::project::SchemaLock, String> {
    read(project_root)
        .map(|existing| bynk_emit::project::SchemaLock::On { existing })
        .map_err(|e| format!("could not read {}: {e}", lock_path(project_root).display()))
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `bynk.schema.lock` to `project_root`, atomically (temp file +
/// `sync_all` + rename + best-effort directory fsync, so a crash mid-write
/// can only leave the intact old file or the intact new one — `ledger.rs`'s
/// own discipline). A no-op when `content` is byte-identical to what is
/// already on disk, so a clean rebuild never touches the file's mtime or
/// produces a spurious `git diff`.
pub fn write(project_root: &Path, content: &str) -> io::Result<()> {
    write_named(project_root, LOCK_FILE, content)
}

/// [`write()`] for any lock file at the project root (#2554 writes
/// `bynk.contracts.lock` with the same atomic discipline).
pub(crate) fn write_named(project_root: &Path, file: &str, content: &str) -> io::Result<()> {
    let path = project_root.join(file);
    if let Ok(existing) = std::fs::read_to_string(&path)
        && existing == content
    {
        return Ok(());
    }

    let dir = project_root;
    let (tmp, mut file) = loop {
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!(".{file}.{}.{n}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => break (candidate, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };

    let write_then_sync = file
        .write_all(content.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(e) = write_then_sync {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(file);
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Ok(dir_handle) = std::fs::File::open(dir) {
        let _ = dir_handle.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway on-disk directory, removed on drop (including on panic) —
    /// mirrors this crate's other test modules' own `Scratch`.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch_dir(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "bynk_1078_schema_lock_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    #[test]
    fn read_of_an_absent_file_is_verified_absent() {
        let dir = scratch_dir("absent");
        assert_eq!(read(&dir.0).unwrap(), None);
    }

    /// #1085 review: the property `SchemaLock`'s `On { existing }` exists to
    /// protect — a present-but-unreadable lock file must be a real error, not
    /// silently degrade to "verified absent" (which would re-baseline a real
    /// registry's history). Unix-only: `chmod`-based permission denial has no
    /// portable Windows equivalent, and this crate's other permission-style
    /// tests are unix-gated the same way.
    #[cfg(unix)]
    #[test]
    fn read_of_an_unreadable_present_file_is_a_real_error_not_absent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch_dir("unreadable");
        std::fs::write(lock_path(&dir.0), "version = 1\n").unwrap();
        std::fs::set_permissions(lock_path(&dir.0), std::fs::Permissions::from_mode(0o000))
            .unwrap();

        // Skip if the test runs as root (or another privileged context) where
        // permission bits don't actually block the read.
        if std::fs::read_to_string(lock_path(&dir.0)).is_ok() {
            return;
        }
        assert!(
            read(&dir.0).is_err(),
            "an unreadable-but-present file must not be reported as absent"
        );
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = scratch_dir("roundtrip");
        let content = "version = 1\n";
        write(&dir.0, content).unwrap();
        assert_eq!(read(&dir.0).unwrap().as_deref(), Some(content));
    }

    #[test]
    fn write_is_a_no_op_when_content_is_unchanged() {
        let dir = scratch_dir("noop-write");
        let content = "version = 1\n";
        write(&dir.0, content).unwrap();
        let mtime_before = std::fs::metadata(lock_path(&dir.0))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        write(&dir.0, content).unwrap();
        let mtime_after = std::fs::metadata(lock_path(&dir.0))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "unchanged content must not rewrite the file"
        );
    }
}
