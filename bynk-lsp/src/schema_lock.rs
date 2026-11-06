//! #2081: the project's `bynk.schema.lock`, read **read-only** for the
//! editor's analysis rounds, so the editor reports a non-additive event change
//! (`bynk.event.non_additive_schema_change`) as `bynk check` does.
//!
//! The read lives here, above the driver boundary, like the rest of this
//! crate's disk reads (`content.rs`): `bynk-ide` and `bynk-check` take the
//! pre-read content as a [`SchemaLock`] and touch no disk for it.
//!
//! Cheap on every keystroke: [`SchemaLockCache::current`] keeps the last
//! content with the file's `(mtime, len)` stamp and only re-reads when the
//! stamp moves, so an unchanged lock costs one `stat` per round. The
//! `**/bynk.schema.lock` file watcher also clears the cache and schedules a
//! round, so an edit to the lock (a `git checkout`, a `bynk compile`) shows
//! without waiting for the next keystroke, and an edit within one mtime tick
//! is still seen.
//!
//! Nothing here ever writes the lock.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bynk_ide::SchemaLock;

/// The lock's file name, at the project root.
pub(crate) const LOCK_FILE: &str = "bynk.schema.lock";

/// Where a project's lock lives.
pub(crate) fn lock_path(project_root: &Path) -> PathBuf {
    project_root.join(LOCK_FILE)
}

/// What a `stat` says about the file: its modification time and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// One project's cached lock. `Default` is empty: the next
/// [`current`](Self::current) reads the file.
#[derive(Clone, Debug, Default)]
pub(crate) struct SchemaLockCache {
    /// The stamp the cached value was read at (`None`: the file was absent)
    /// and the value itself.
    entry: Option<(Option<Stamp>, SchemaLock)>,
}

impl SchemaLockCache {
    /// Forget the cached value, so the next [`current`](Self::current)
    /// re-reads the file. The file watcher calls this on a lock change.
    pub(crate) fn invalidate(&mut self) {
        self.entry = None;
    }

    /// The lock for `project_root` as the analysis should see it.
    ///
    /// - present: [`SchemaLock::On`] with its content;
    /// - absent: [`SchemaLock::Off`]. A fresh project has nothing to compare
    ///   against, and `On { existing: None }` would only baseline every event
    ///   silently, so the editor skips the sweep instead;
    /// - unreadable for any other reason: [`SchemaLock::Off`] with a logged
    ///   warning, and nothing cached, so the next round tries again. An
    ///   editor round never fails over the lock.
    pub(crate) fn current(&mut self, project_root: &Path) -> SchemaLock {
        let path = lock_path(project_root);
        let stamp = match std::fs::metadata(&path) {
            Ok(meta) => Some(Stamp {
                modified: meta.modified().ok(),
                len: meta.len(),
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!("could not stat {}: {e}", path.display());
                self.entry = None;
                return SchemaLock::Off;
            }
        };
        if let Some((cached, lock)) = &self.entry
            && *cached == stamp
        {
            return lock.clone();
        }
        let lock = match stamp {
            None => SchemaLock::Off,
            Some(_) => match std::fs::read_to_string(&path) {
                Ok(text) => SchemaLock::On {
                    existing: Some(text),
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => SchemaLock::Off,
                Err(e) => {
                    tracing::warn!("could not read {}: {e}", path.display());
                    self.entry = None;
                    return SchemaLock::Off;
                }
            },
        };
        self.entry = Some((stamp, lock.clone()));
        lock
    }
}

/// Whether a watched-file URI is a project's `bynk.schema.lock`.
pub(crate) fn is_schema_lock(uri: &tower_lsp::lsp_types::Url) -> bool {
    let Ok(path) = uri.to_file_path() else {
        return false;
    };
    path.file_name().and_then(|n| n.to_str()) == Some(LOCK_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory, removed on drop (including on panic).
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "bynk_lsp_2081_schema_lock_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn existing(lock: &SchemaLock) -> Option<&str> {
        match lock {
            SchemaLock::On { existing } => existing.as_deref(),
            SchemaLock::Off => None,
        }
    }

    #[test]
    fn an_absent_lock_is_off() {
        let dir = scratch("absent");
        let mut cache = SchemaLockCache::default();
        assert_eq!(cache.current(&dir.0), SchemaLock::Off);
    }

    #[test]
    fn a_present_lock_is_on_with_its_content() {
        let dir = scratch("present");
        std::fs::write(lock_path(&dir.0), "version = 1\n").unwrap();
        let mut cache = SchemaLockCache::default();
        assert_eq!(existing(&cache.current(&dir.0)), Some("version = 1\n"));
    }

    #[test]
    fn an_unchanged_stamp_serves_the_cached_content() {
        let dir = scratch("cached");
        std::fs::write(lock_path(&dir.0), "version = 1\n").unwrap();
        let mut cache = SchemaLockCache::default();
        cache.current(&dir.0);
        // Same stamp, different cached value: what comes back proves the
        // file was not re-read.
        if let Some((_, lock)) = &mut cache.entry {
            *lock = SchemaLock::On {
                existing: Some("from the cache\n".into()),
            };
        }
        assert_eq!(existing(&cache.current(&dir.0)), Some("from the cache\n"));
    }

    #[test]
    fn a_changed_file_is_re_read() {
        let dir = scratch("changed");
        std::fs::write(lock_path(&dir.0), "version = 1\n").unwrap();
        let mut cache = SchemaLockCache::default();
        cache.current(&dir.0);
        // A different length moves the stamp even within one mtime tick.
        std::fs::write(lock_path(&dir.0), "version = 1\n# edited\n").unwrap();
        assert_eq!(
            existing(&cache.current(&dir.0)),
            Some("version = 1\n# edited\n")
        );
        std::fs::remove_file(lock_path(&dir.0)).unwrap();
        assert_eq!(cache.current(&dir.0), SchemaLock::Off);
    }

    #[test]
    fn invalidate_forces_a_re_read_at_the_same_stamp() {
        let dir = scratch("invalidate");
        std::fs::write(lock_path(&dir.0), "version = 1\n").unwrap();
        let mut cache = SchemaLockCache::default();
        cache.current(&dir.0);
        if let Some((_, lock)) = &mut cache.entry {
            *lock = SchemaLock::Off;
        }
        cache.invalidate();
        assert_eq!(existing(&cache.current(&dir.0)), Some("version = 1\n"));
    }

    #[test]
    fn reading_never_writes_the_lock() {
        let dir = scratch("readonly");
        let mut cache = SchemaLockCache::default();
        cache.current(&dir.0);
        assert!(!lock_path(&dir.0).exists(), "an absent lock stays absent");
    }

    #[test]
    fn only_the_lock_file_name_matches() {
        // A host-absolute base, so `from_file_path` succeeds on Windows too.
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        let yes = tower_lsp::lsp_types::Url::from_file_path(base.join(LOCK_FILE)).unwrap();
        let no =
            tower_lsp::lsp_types::Url::from_file_path(base.join("not-bynk.schema.lock")).unwrap();
        assert!(is_schema_lock(&yes));
        assert!(!is_schema_lock(&no));
    }
}
