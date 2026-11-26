//! Per-PR attribution of gated-probe moves (#4428).
//!
//! A gated greenfield-status probe a change moves must be named in that change's own pending
//! file: `probes:` for any move, and `raises:` for a growth probe that rises. The table gate
//! (`greenfield_status_table_is_current`) cannot say whose move a disagreement is: it excuses a
//! probe when *any* pending file in the tree names it, and the tree holds every increment merged
//! since the last stamp. So a PR that moved a probe without naming it was excused by another PR's
//! file, passed CI and the merge queue, and went red only on the stamp that consumed that other
//! file (#4415, #4421).
//!
//! This module reads a PR's own moves instead: the gated probes' live readings on the base
//! revision against the PR's tree (the probes are file scans, so a checkout of the base is all
//! the first needs), and the pending files the PR itself added or changed against the base's.
//! A stamp PR deletes pending files and rewrites the table but changes no code, so it moves no
//! probe and owes nothing: the stamp path is unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::greenfield_status::{GROWTH_PROBES, Probe, leading_count};

/// A gated probe whose reading differs between the base revision and the PR's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub probe: &'static str,
    pub from: String,
    pub to: String,
}

/// The probes the PR's own pending files declare: `probes:` (a move) and `raises:` (a rise).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Declared {
    pub probes: BTreeSet<String>,
    pub raised: BTreeSet<String>,
}

/// Every probe in `pr` whose reading differs from `base`'s. A probe `base` has no reading for is
/// newly introduced by the PR and has nothing to have moved from, as for the growth check.
pub fn moved(base: &[Probe], pr: &[Probe]) -> Vec<Moved> {
    pr.iter()
        .filter_map(|now| {
            let was = base.iter().find(|p| p.name == now.name)?;
            (was.reads != now.reads).then(|| Moved {
                probe: now.name,
                from: was.reads.clone(),
                to: now.reads.clone(),
            })
        })
        .collect()
}

/// The pending files a PR owns: those in `tree` that `base` does not have, or has with other
/// text. Both are `(file name, text)`, `README.md` excluded. A file the PR only deletes (a stamp
/// consuming it) is in neither result.
pub fn own_pending_files(
    tree: &[(String, String)],
    base: &[(String, String)],
) -> Vec<(String, String)> {
    let base: BTreeMap<&str, &str> = base
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    tree.iter()
        .filter(|(name, text)| base.get(name.as_str()) != Some(&text.as_str()))
        .cloned()
        .collect()
}

/// What `own` declares. A file that does not parse is an error naming it, never an empty
/// declaration: a PR must not look attributed because its own file is malformed.
pub fn declared_by(own: &[(String, String)]) -> Result<Declared, Vec<String>> {
    let mut declared = Declared::default();
    let mut errors = Vec::new();
    for (name, text) in own {
        match crate::validate(name, text) {
            Ok(p) => {
                declared.probes.extend(p.probes);
                declared
                    .raised
                    .extend(p.raises.into_iter().map(|r| r.probe));
            }
            Err(errs) => errors.extend(errs.into_iter().map(|e| format!("{name}: {e}"))),
        }
    }
    if errors.is_empty() {
        Ok(declared)
    } else {
        Err(errors)
    }
}

/// The growth probes in `pr` whose leading count is above `base`'s: a rise this PR makes.
pub fn rises(base: &[Probe], pr: &[Probe]) -> Vec<(&'static str, usize, usize)> {
    GROWTH_PROBES
        .iter()
        .filter_map(|name| {
            let now = leading_count(&pr.iter().find(|p| p.name == *name)?.reads)?;
            let was = leading_count(&base.iter().find(|p| p.name == *name)?.reads)?;
            (now > was).then_some((*name, was, now))
        })
        .collect()
}

/// Whether `pr`'s tree owes the pending files it has: `Ok` when every probe it moved is named in
/// its own `probes:` and every growth probe it raised in its own `raises:`. `tree` and `base` are
/// each side's `(file name, text)` pending files; a probe only another file names is reported as
/// such, since that is what excused the move before.
///
/// A move the PR recorded itself, by rewriting the probe's row in the committed table to the new
/// reading (`cargo xtask greenfield-status --apply`, the route for a PR with no increment), is its
/// own too: `tables` is the table's text at the base and in the PR's tree.
pub fn check(
    base_live: &[Probe],
    pr_live: &[Probe],
    tree: &[(String, String)],
    base: &[(String, String)],
    tables: (&str, &str),
) -> Result<(), String> {
    let own = own_pending_files(tree, base);
    let declared = declared_by(&own).map_err(|errs| {
        format!(
            "a pending file this PR adds or changes is invalid, so no `probes:`/`raises:` could \
             be read. Fix it first (run `cargo xtask check-pending`):\n{}",
            errs.join("\n")
        )
    })?;
    let elsewhere: BTreeSet<String> = tree
        .iter()
        .filter_map(|(name, text)| crate::validate(name, text).ok())
        .flat_map(|p| {
            p.probes
                .into_iter()
                .chain(p.raises.into_iter().map(|r| r.probe))
        })
        .collect();
    let note = |probe: &str| {
        if elsewhere.contains(probe) {
            " (another pending file names it, which does not excuse this PR)"
        } else {
            ""
        }
    };

    let mut problems = Vec::new();
    for m in moved(base_live, pr_live) {
        if !declared.probes.contains(m.probe) && !rewrote_row(tables, &m) {
            problems.push(format!(
                "  {} moved {:?} -> {:?}; name it in `probes:` of a pending file this PR adds{}",
                m.probe,
                m.from,
                m.to,
                note(m.probe)
            ));
        }
    }
    for (probe, from, to) in rises(base_live, pr_live) {
        if !declared.raised.contains(probe) {
            problems.push(format!(
                "  {probe} rose {from} -> {to}; add `raises: {probe} — <reason>` to a pending \
                 file this PR adds{}",
                note(probe)
            ));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "this PR moves gated probe(s) its own pending files do not name. A probe another \
         increment's file names is not excused for this PR: the stamp consumes that file, and \
         the move then surfaces on a stamp PR instead of here. Put `probes: <probe>` (and, for a \
         growth probe that rises, `raises: <probe> — <reason>`) in the pending file of the PR \
         that moves it:\n{}",
        problems.join("\n")
    ))
}

/// Whether the PR itself moved `m`'s row in the committed table to the new reading: the tree's
/// row is `m.to` and the base's is not. A row someone else already moved to it is not the PR's.
fn rewrote_row((base_table, tree_table): (&str, &str), m: &Moved) -> bool {
    use crate::greenfield_status::committed_reading;
    committed_reading(tree_table, m.probe) == Some(m.to.as_str())
        && committed_reading(base_table, m.probe) != Some(m.to.as_str())
}

/// A temporary checkout of a revision, removed when dropped.
pub struct BaseTree {
    repo: PathBuf,
    dir: PathBuf,
}

impl BaseTree {
    /// Check out `rev` of the repository at `repo` into a fresh temporary directory
    /// (`git worktree add --detach`, so it needs only git and the commit, not a full history).
    pub fn checkout(repo: &Path, rev: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("xtask-base-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = Command::new("git")
            .args(["worktree", "add", "--detach", "--force"])
            .arg(&dir)
            .arg(rev)
            .current_dir(repo)
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "cannot check out {rev}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(Self {
            repo: repo.to_path_buf(),
            dir,
        })
    }

    /// The checkout's root.
    pub fn root(&self) -> &Path {
        &self.dir
    }
}

impl Drop for BaseTree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.dir)
            .current_dir(&self.repo)
            .output();
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&self.repo)
            .output();
    }
}

/// The pending files under `root`, `(file name, text)`, `README.md` excluded; empty when there is
/// no directory.
pub fn read_pending(root: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(root.join("design/pending")) else {
        return Vec::new();
    };
    let mut files: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".md") || name == "README.md" {
                return None;
            }
            Some((name, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(name: &'static str, reads: &str) -> Probe {
        Probe {
            name,
            gated: true,
            reads: reads.to_string(),
        }
    }

    fn file(name: &str, front: &str) -> (String, String) {
        (
            name.to_string(),
            format!("---\nlevel: patch\nchangelog: x\n{front}---\n"),
        )
    }

    /// #4415: a new diagnostic moved three diagnostics probes; its PR named none of them, and the
    /// probes were excused because other pending files on `main` named them.
    #[test]
    fn a_move_only_another_pending_file_names_is_refused() {
        let others = file(
            "l5-other.md",
            "probes: emit_diagnostics, diagnostic_coverage, explained_codes\n",
        );
        let mine = file("l5-3899-d.md", "");
        let base = [probe("emit_diagnostics", "800/880")];
        let pr = [probe("emit_diagnostics", "801/881")];
        // The old, whole-tree reading: the move is excused by the other file.
        let tree = vec![others.clone(), mine.clone()];
        let tree_named: BTreeSet<String> = tree
            .iter()
            .filter_map(|(n, t)| crate::validate(n, t).ok())
            .flat_map(|p| p.probes)
            .collect();
        assert!(tree_named.contains("emit_diagnostics"));
        // The attributed reading: this PR's own files do not name it.
        let e = check(&base, &pr, &tree, std::slice::from_ref(&others), ("", "")).unwrap_err();
        assert!(
            e.contains("emit_diagnostics moved \"800/880\" -> \"801/881\""),
            "{e}"
        );
        assert!(e.contains("another pending file names it"), "{e}");
        // Naming it in the PR's own file attributes it.
        let named = file("l5-3899-d.md", "probes: emit_diagnostics\n");
        let tree = vec![others.clone(), named];
        assert!(check(&base, &pr, &tree, &[others], ("", "")).is_ok());
    }

    /// #4421: a PR raised `ts_writes` by one with no `probes:` or `raises:` line, and main stayed
    /// green because another pending file carried `raises: ts_writes`.
    #[test]
    fn a_rise_only_another_pending_file_raises_is_refused() {
        let others = file(
            "l8-other.md",
            "probes: ts_writes\nraises: ts_writes — new emitter site (#1)\n",
        );
        let base = [probe("ts_writes", "475")];
        let pr = [probe("ts_writes", "476")];
        let base_files = vec![others.clone()];

        // No declaration of its own.
        let tree = vec![others.clone(), file("l2-4421.md", "")];
        let e = check(&base, &pr, &tree, &base_files, ("", "")).unwrap_err();
        assert!(e.contains("ts_writes moved \"475\" -> \"476\""), "{e}");
        assert!(e.contains("ts_writes rose 475 -> 476"), "{e}");

        // `probes:` alone names the move, not the rise.
        let tree = vec![others.clone(), file("l2-4421.md", "probes: ts_writes\n")];
        let e = check(&base, &pr, &tree, &base_files, ("", "")).unwrap_err();
        assert!(!e.contains("ts_writes moved"), "{e}");
        assert!(e.contains("ts_writes rose 475 -> 476"), "{e}");

        // Both, in the PR's own file: attributed.
        let own = file(
            "l2-4421.md",
            "probes: ts_writes\nraises: ts_writes — one more site (#2)\n",
        );
        let tree = vec![others, own];
        assert!(check(&base, &pr, &tree, &base_files, ("", "")).is_ok());
    }

    /// A stamp PR deletes the pending files it consumes and changes no code: it moves no probe,
    /// owes no declaration, and its tree has no pending file of its own.
    #[test]
    fn a_stamp_pr_that_only_deletes_pending_files_owes_nothing() {
        let consumed = file("l8-old.md", "probes: ts_writes\n");
        let live = [probe("ts_writes", "476")];
        let base_files = vec![consumed];
        assert!(own_pending_files(&[], &base_files).is_empty());
        assert!(check(&live, &live, &[], &base_files, ("", "")).is_ok());
    }

    /// A PR with no increment commits the table itself (`cargo xtask greenfield-status --apply`):
    /// the move is its own, recorded in its row, and needs no pending file.
    #[test]
    fn a_move_the_pr_recorded_in_the_table_is_its_own() {
        let row = |reading: &str| format!("| `ts_any` | yes | {reading} |\n");
        let base = [probe("ts_any", "21")];
        let pr = [probe("ts_any", "20")];
        // The PR rewrote the row to the new reading: attributed, no pending file needed.
        assert!(check(&base, &pr, &[], &[], (&row("21"), &row("20"))).is_ok());
        // The row still says the old reading: not recorded, not named: refused.
        assert!(check(&base, &pr, &[], &[], (&row("21"), &row("21"))).is_err());
        // The base's row already said the new reading (another change moved it): not this PR's.
        assert!(check(&base, &pr, &[], &[], (&row("20"), &row("20"))).is_err());
        // A hand-edited row to something the probe does not read does not excuse it.
        assert!(check(&base, &pr, &[], &[], (&row("21"), &row("19"))).is_err());
    }

    #[test]
    fn a_pr_that_moves_no_probe_owes_no_pending_file() {
        let live = [probe("ts_writes", "475"), probe("ts_any", "21")];
        assert!(check(&live, &live, &[], &[], ("", "")).is_ok());
    }

    #[test]
    fn a_pending_file_is_the_prs_own_when_added_or_changed_and_not_when_unchanged() {
        let same = file("a.md", "probes: ts_any\n");
        let edited_then = file("b.md", "");
        let edited_now = file("b.md", "probes: ts_any\n");
        let added = file("c.md", "");
        let base = vec![same.clone(), edited_then];
        let tree = vec![same, edited_now.clone(), added.clone()];
        assert_eq!(own_pending_files(&tree, &base), vec![edited_now, added]);
    }

    #[test]
    fn a_probe_with_no_reading_at_the_base_is_new_and_has_not_moved() {
        let pr = [probe("brand_new_probe", "3")];
        assert!(moved(&[], &pr).is_empty());
        assert!(rises(&[], &pr).is_empty());
    }

    #[test]
    fn an_invalid_own_pending_file_is_reported_not_read_as_empty() {
        let bad = ("l2-bad.md".to_string(), "not a pending file".to_string());
        let base = [probe("ts_writes", "475")];
        let pr = [probe("ts_writes", "475")];
        let e = check(&base, &pr, &[bad], &[], ("", "")).unwrap_err();
        assert!(e.contains("l2-bad.md"), "{e}");
        assert!(e.contains("Fix it first"), "{e}");
    }

    #[test]
    fn a_fall_needs_no_raises_but_still_needs_probes() {
        let own = file("l2-x.md", "probes: ts_writes\n");
        let base = [probe("ts_writes", "475")];
        let pr = [probe("ts_writes", "470")];
        assert!(check(&base, &pr, &[own], &[], ("", "")).is_ok());
        let e = check(&base, &pr, &[file("l2-x.md", "")], &[], ("", "")).unwrap_err();
        assert!(e.contains("ts_writes moved"), "{e}");
        assert!(!e.contains("rose"), "{e}");
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The base tree is the revision's files, whatever the working tree has since become, and it
    /// is gone when dropped.
    #[test]
    fn the_base_tree_is_the_revision_and_is_removed_when_dropped() {
        let repo = std::env::temp_dir().join(format!("xtask-attribution-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join("design/pending")).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("design/pending/a.md"), "one").unwrap();
        std::fs::write(repo.join("code.rs"), "base").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let base = {
            let out = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repo)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        // The working tree moves on.
        std::fs::write(repo.join("code.rs"), "changed").unwrap();
        std::fs::write(repo.join("design/pending/b.md"), "two").unwrap();

        let dir = {
            let tree = BaseTree::checkout(&repo, &base).unwrap();
            assert_eq!(
                std::fs::read_to_string(tree.root().join("code.rs")).unwrap(),
                "base"
            );
            assert_eq!(
                read_pending(tree.root()),
                vec![("a.md".to_string(), "one".to_string())]
            );
            tree.root().to_path_buf()
        };
        assert!(!dir.exists(), "the checkout is removed on drop");
        assert_eq!(
            read_pending(&repo).len(),
            2,
            "the repository's own tree is untouched"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}
