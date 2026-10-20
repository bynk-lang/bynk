//! Content-ownership track (#1086): a complete `(path, content)` map for a
//! project's `.bynk` files — the open-buffer overlay first, a real disk read
//! for everything else `bynk_ide::discover_files` names. Consumed by
//! `Backend::project_content`, the sole project-enumeration entry point
//! (slice 0's `Backend::project_files`, bare paths, was deleted once slice 1
//! migrated its last callers) — feeds `bynk-ide`'s content-supplying
//! `for_each_unit`/`resolve_label`/cross-file symbol lookups (completion,
//! signature help, hover rungs 5, 9 and 10, go-to-declaration). No code in this
//! crate reads a project file straight off disk itself any more; an unsaved
//! edit to file A is visible from file B everywhere, closing the staleness
//! this track exists to fix.
//!
//! Implementation note (slice 0, found only under implementation, not
//! anticipated by the track doc's settling pass): `bynk_ide::discover_files`
//! already resolves a project's `include`/`exclude` trees and enumerates its
//! `.bynk` files with no `bynk-emit` type crossing into `bynk-lsp` — already
//! in production use by the since-deleted `Backend::project_files`. No new
//! `bynk-ide` type was needed to close slice 0; see
//! `design/decisions/0325-content-ownership-seam-simplification.md`, which
//! supersedes ADR 0322's `ProjectDirs`/`resolve_dirs` design.

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_ide::AnalysisRoots;

/// `overlay` wins for any file it already covers (an open buffer, keyed by
/// canonicalised absolute path — the same convention
/// `Backend::run_project_diagnostics` uses). Every other `.bynk` file
/// `bynk_ide::discover_files` names for `roots` is read fresh from disk. A
/// file that fails to read (deleted between discovery and read, a permission
/// error) is silently omitted — the same behaviour `bynk-emit`'s own
/// `read_source` fallback has today for an unreadable path.
///
/// Content-ownership track (#1086) slice 5 correction: also resolves
/// `roots`'s own `bynk.toml` (`overlay`'s entry if present — an unsaved
/// manifest edit — else a real disk read) and includes it in the returned
/// map. `bynk-ide` can no longer do this itself (R2.3 forbids `std::fs`
/// below the driver, and slice 5 deleted `bynk-emit`'s disk-read fallback
/// that used to paper over this), so the caller — this crate, above the
/// driver boundary — is where `Sources` gets constructed, manifest
/// included, exactly as R2.3 intends. Without this, both this sweep's own
/// enumeration *and* any later `diagnose_project_with(&roots, &content)`
/// call over the returned map would silently degrade to
/// `ProjectPaths::conventional` for a non-conventional project.
pub(crate) fn sweep_project_content(
    roots: &AnalysisRoots,
    overlay: &HashMap<PathBuf, String>,
) -> HashMap<PathBuf, String> {
    let mut content = overlay.clone();
    if let AnalysisRoots::Project(root) = roots {
        let toml_path = root.join("bynk.toml");
        if !content.contains_key(&toml_path) {
            let canonical = toml_path
                .canonicalize()
                .unwrap_or_else(|_| toml_path.clone());
            if !content.contains_key(&canonical)
                && let Ok(text) = std::fs::read_to_string(&toml_path)
            {
                content.insert(toml_path, text);
            }
        }
    }
    for path in bynk_ide::discover_files(roots, &content) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if content.contains_key(&canonical) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            content.insert(canonical, text);
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bynk-lsp-content-sweep-test-{test_name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");
        for (rel, contents) in files {
            let p = root.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, contents).expect("write file");
        }
        root.canonicalize().expect("canonicalize test root")
    }

    #[test]
    fn reads_every_discovered_file_not_already_in_the_overlay() {
        let root = setup_project(
            "reads-disk",
            &[("src/a.bynk", "commons a\n"), ("src/b.bynk", "commons b\n")],
        );
        let roots = AnalysisRoots::Project(root.clone());
        let content = sweep_project_content(&roots, &HashMap::new());
        assert_eq!(content.len(), 2, "expected both files, got {content:?}");
        let a_path = root.join("src/a.bynk").canonicalize().unwrap();
        assert_eq!(
            content.get(&a_path).map(String::as_str),
            Some("commons a\n")
        );
    }

    #[test]
    fn overlay_wins_over_disk_for_a_covered_file() {
        let root = setup_project("overlay-wins", &[("src/a.bynk", "commons a\n")]);
        let a_path = root.join("src/a.bynk").canonicalize().unwrap();
        let mut overlay = HashMap::new();
        overlay.insert(a_path.clone(), "commons a // unsaved edit\n".to_string());
        let roots = AnalysisRoots::Project(root.clone());
        let content = sweep_project_content(&roots, &overlay);
        assert_eq!(
            content.get(&a_path).map(String::as_str),
            Some("commons a // unsaved edit\n"),
            "the open buffer's content must win over the file's on-disk content"
        );
    }

    #[test]
    fn overlay_only_file_with_no_disk_counterpart_is_still_included() {
        // A brand-new, not-yet-saved file `discover_files` can't find on disk.
        let root = setup_project("overlay-only", &[("src/a.bynk", "commons a\n")]);
        let ghost_path = root.join("src/ghost.bynk");
        let mut overlay = HashMap::new();
        overlay.insert(ghost_path.clone(), "commons ghost\n".to_string());
        let roots = AnalysisRoots::Project(root.clone());
        let content = sweep_project_content(&roots, &overlay);
        assert_eq!(
            content.len(),
            2,
            "expected a.bynk (disk) + ghost.bynk (overlay-only), got {content:?}"
        );
        assert_eq!(
            content.get(&ghost_path).map(String::as_str),
            Some("commons ghost\n")
        );
    }

    #[test]
    fn single_tree_mode_sweeps_the_tree_with_no_manifest() {
        let root = setup_project("single-tree", &[("a.bynk", "commons a\n")]);
        let roots = AnalysisRoots::SingleTree(root.clone());
        let content = sweep_project_content(&roots, &HashMap::new());
        let a_path = root.join("a.bynk").canonicalize().unwrap();
        assert_eq!(
            content.get(&a_path).map(String::as_str),
            Some("commons a\n")
        );
    }
}
