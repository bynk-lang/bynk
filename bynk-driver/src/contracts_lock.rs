//! Reading and atomically writing `bynk.contracts.lock` (#2554): the driver's
//! side of [`CompileOptions::contracts_lock`], beside [`crate::schema_lock`]
//! and sharing its read and write.
//!
//! The pure document (`ContractsLock`, parse, serialize) is
//! `bynk_project::contracts_lock`; a build only reads the file, and `bynk
//! deploy --accept-compatible-skew` is what writes it.
//!
//! [`CompileOptions::contracts_lock`]: bynk_emit::project::CompileOptions::contracts_lock

use std::io;
use std::path::{Path, PathBuf};

/// The file at the project root.
pub const LOCK_FILE: &str = "bynk.contracts.lock";

/// Where the lock lives for a project root, for messages that name the file.
pub fn lock_path(project_root: &Path) -> PathBuf {
    project_root.join(LOCK_FILE)
}

/// The lock's raw content; `Ok(None)` is verified absent, any other failure
/// is an error. See [`crate::schema_lock::read`] for why the two differ.
pub fn read(project_root: &Path) -> io::Result<Option<String>> {
    crate::schema_lock::read_named(project_root, LOCK_FILE)
}

/// Write the lock atomically; a no-op when the content is unchanged.
pub fn write(project_root: &Path, content: &str) -> io::Result<()> {
    crate::schema_lock::write_named(project_root, LOCK_FILE, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_lock_is_verified_absent_and_a_written_one_reads_back() {
        let dir = std::env::temp_dir().join(format!("bynk_2554_lock_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir).unwrap(), None);
        write(&dir, "version = 1\n").unwrap();
        assert_eq!(read(&dir).unwrap().as_deref(), Some("version = 1\n"));
        assert!(lock_path(&dir).ends_with("bynk.contracts.lock"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
