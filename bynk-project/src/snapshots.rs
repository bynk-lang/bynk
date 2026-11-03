//! #2842: the project's `snapshots/` folder — one `<suite file>.snap.json` per
//! suite file that takes a snapshot (`expect v matches snapshot "name"`).
//!
//! The compiler stays disk-free: the driver lists this folder with
//! [`read_snapshot_files`] and hands the files' text to the build, which inlines
//! each into its test module. Only the driver writes them
//! (`bynk test --update-snapshots`).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// The project's snapshots folder, relative to the project root.
pub const PROJECT_SNAPSHOTS_DIR: &str = "snapshots";

/// The extension a snapshot file carries after its suite file's name.
pub const SNAPSHOT_SUFFIX: &str = ".snap.json";

/// The snapshot file for the suite file at `identity` (its project-relative
/// path): `snapshots/<path without .bynk>.snap.json`, with `/` separators.
pub fn snapshot_path(identity: &Path) -> String {
    let path = identity.to_string_lossy().replace('\\', "/");
    let stem = path.strip_suffix(".bynk").unwrap_or(&path);
    format!("{PROJECT_SNAPSHOTS_DIR}/{stem}{SNAPSHOT_SUFFIX}")
}

/// Every `*.snap.json` under `<root>/snapshots/`, keyed by its project-relative
/// path with `/` separators, with its text. A missing folder is empty; an
/// unreadable file is an error.
pub fn read_snapshot_files(root: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let dir = root.join(PROJECT_SNAPSHOTS_DIR);
    if dir.is_dir() {
        walk(&dir, PROJECT_SNAPSHOTS_DIR, &mut out)?;
    }
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) -> io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<_>>()?;
    entries.sort();
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let key = format!("{prefix}/{name}");
        if path.is_dir() {
            walk(&path, &key, out)?;
        } else if name.ends_with(SNAPSHOT_SUFFIX) {
            out.insert(key, std::fs::read_to_string(&path)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suite_file_maps_to_its_snapshot_file() {
        assert_eq!(
            snapshot_path(Path::new("tests/github/events.bynk")),
            "snapshots/tests/github/events.snap.json"
        );
        assert_eq!(
            snapshot_path(Path::new("tests\\a.bynk")),
            "snapshots/tests/a.snap.json"
        );
    }
}
