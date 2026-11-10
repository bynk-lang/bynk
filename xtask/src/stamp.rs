//! The merge-time stamp (increment-allocation track, Slice 1).
//!
//! On merge, [`plan`] reads the pending files, assigns the next version(s) in
//! merge order and the next ADR number(s), and [`apply`] materialises them:
//! it runs `scripts/bump-version.sh`, inserts the changelog row(s), writes each
//! `design/decisions/NNNN-<slug>.md` and its index row, and deletes the consumed
//! pending files. Deleting what it consumes is what makes a re-run a no-op —
//! which is why version assignment and ADR materialisation are one atomic pass
//! (the entangled-by-delete finding, proposal #689 DECISION A).
//!
//! The version bump is injected (see [`apply`]) so the whole flow is testable on
//! a fixture tree without running the real, side-effect-heavy bump script.

use crate::{Adr, Level, validated_pending_in};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A semantic version `MAJOR.MINOR.PATCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// The next version for `level`: `minor` bumps MINOR and resets PATCH,
    /// `patch` bumps PATCH. (MAJOR is 0 pre-1.0 and never moved here.)
    pub fn next(self, level: &Level) -> Version {
        match level {
            Level::Minor => Version {
                minor: self.minor + 1,
                patch: 0,
                ..self
            },
            Level::Patch => Version {
                patch: self.patch + 1,
                ..self
            },
        }
    }

    /// Parse an `X.Y.Z` string.
    pub fn parse(s: &str) -> Option<Version> {
        let mut it = s.trim().split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Version {
    /// The ADR/index version string, following the corpus convention: a MINOR
    /// increment is `MAJOR.MINOR` (`0.186`), a PATCH increment keeps its patch
    /// (`0.185.1`, like the existing `v0.29.1`). Since [`next`](Version::next)
    /// resets PATCH to 0 for a minor bump, `patch == 0` is exactly the minor case.
    pub fn short(self) -> String {
        if self.patch == 0 {
            format!("{}.{}", self.major, self.minor)
        } else {
            self.to_string()
        }
    }
}

/// One pending increment stamped with the version it will ship as.
#[derive(Debug)]
pub struct Stamped {
    /// The pending file that produced this (relative name under `design/pending/`).
    pub file: String,
    pub version: Version,
    pub changelog: String,
    pub adrs: Vec<Adr>,
    /// Greenfield reference rule ids this increment closes (#1001) — already
    /// validated (syntax and existence) by `validated_pending_in`.
    pub closes_rule: Vec<String>,
    /// Gated greenfield-status probes this increment acknowledges moving — the
    /// stamp rewrites only these rows of the table.
    pub probes: Vec<String>,
    /// The increment's breaking-change note (#3178), if it declared one; the
    /// stamp writes it, under this increment's version, into the Book's
    /// `reference/upgrading.md`.
    pub breaking: Option<String>,
}

/// The full stamp plan for the pending files present.
#[derive(Debug)]
pub struct Plan {
    pub base_version: Version,
    pub increments: Vec<Stamped>,
    /// The next free ADR number, for materialisation.
    pub first_adr_number: u32,
}

impl Plan {
    /// The version the manifests end on — the last increment's, or the base if
    /// there is nothing to stamp.
    pub fn final_version(&self) -> Version {
        self.increments
            .last()
            .map(|s| s.version)
            .unwrap_or(self.base_version)
    }

    pub fn is_empty(&self) -> bool {
        self.increments.is_empty()
    }
}

/// Build the plan for `root`: read the workspace version and the pending files,
/// and assign each pending file (in filename order — a deterministic proxy for
/// merge order) its own next version. Returns validation errors verbatim.
pub fn plan(root: &Path) -> Result<Plan, Vec<String>> {
    let cargo = root.join("Cargo.toml");
    let cargo_src = fs::read_to_string(&cargo)
        .map_err(|e| vec![format!("cannot read {}: {e}", cargo.display())])?;
    let base_version = parse_workspace_version(&cargo_src)
        .ok_or_else(|| vec![format!("no `version = \"X.Y.Z\"` in {}", cargo.display())])?;

    let pending = validated_pending_in(root)?;
    let first_adr_number = next_adr_number(root).map_err(|e| {
        vec![format!(
            "cannot scan {}: {e}",
            root.join("design/decisions").display()
        )]
    })?;

    let mut version = base_version;
    let mut increments = Vec::new();
    for (file, p) in pending {
        version = version.next(&p.level);
        increments.push(Stamped {
            file,
            version,
            changelog: p.changelog,
            adrs: p.adrs,
            closes_rule: p.closes_rule,
            probes: p.probes,
            breaking: p.breaking,
        });
    }

    Ok(Plan {
        base_version,
        increments,
        first_adr_number,
    })
}

/// Apply `plan` to `root`: write ADR files + index rows, insert changelog rows,
/// run `bump` for the final version, then delete the consumed pending files.
///
/// The whole thing is transactional. Every write is *staged* in memory first —
/// the changelog and index edits are computed, ADR contents rendered, and any
/// ADR-number collision detected — before a single byte hits disk, so a
/// malformed table or a stale number fails with nothing written. During the
/// commit phase the tree is mutated in a fixed order (ADR files, changelog,
/// index, then the flaky `bump`); if any step fails, the tree is *rolled back*
/// to its pre-apply state — the edited docs restored and the freshly written
/// ADR files removed. Only once everything has succeeded are the consumed
/// pending files deleted.
///
/// This is what makes a re-run safe: a failed run (a `bump` that errors, most
/// likely) leaves the pending files intact *and* no partial changelog/ADR edits
/// behind, so a retry recomputes the identical plan rather than duplicating rows
/// or re-numbering ADRs past the files a previous attempt wrote. (`bump`'s own
/// partial effects on the manifests it rewrites are outside this function's
/// reach; the CLI additionally refuses to `--apply` on a dirty worktree so that
/// debris is surfaced too.)
///
/// `bump` is injected so tests exercise the whole flow on a fixture tree with a
/// stub that just rewrites the fixture's `Cargo.toml`. `pr_number` is the PR
/// whose merge triggered *this run*, if it could be recovered from the
/// triggering commit message (#1001) — `None` records a ledger row without
/// one (e.g. a local `stamp --apply` run outside CI) rather than guessing.
/// Only trusted for a single-increment plan: a multi-increment run (a prior
/// run's failed push left its pending file for this one to pick up) has no
/// way to tell which increment the recovered number actually belongs to, so
/// every row in that case gets a blank cell instead of one confidently wrong
/// number applied to increments from different PRs.
pub fn apply(
    root: &Path,
    plan: &Plan,
    pr_number: Option<u32>,
    bump: impl Fn(Version) -> io::Result<()>,
) -> io::Result<()> {
    if plan.is_empty() {
        return Ok(());
    }

    let decisions = root.join("design/decisions");
    let changelog_path = root.join("site/src/content/docs/book/reference/changelog.md");
    let readme_path = decisions.join("README.md");
    let rule_ledger_path = root.join("design/greenfield-status-rules.md");
    let upgrading_path = root.join(crate::release_notes::UPGRADING_PAGE);

    // `pr_number` names the PR whose merge triggered *this run*, not any one
    // increment in `plan` — those are usually the same increment, but not
    // always: a stamp run that exhausts its push retries (`stamp.yml`) exits
    // without consuming its pending file, so the *next* run's plan can carry
    // two increments from two different PRs. Attributing both to the second
    // PR would be a confidently wrong provenance record — worse than the
    // blank cell this design already accepts for "couldn't recover a PR
    // number" — so the recovered number is only trusted when it can only mean
    // one increment.
    let ledger_pr_number = if plan.increments.len() == 1 {
        pr_number
    } else {
        None
    };

    // --- Stage: compute every write in memory. Nothing is on disk yet, so a
    // fault here (a missing table anchor, an ADR-number collision) aborts with
    // the tree untouched. ---
    let mut number = plan.first_adr_number;
    let mut adr_files: Vec<(PathBuf, String)> = Vec::new();
    let mut changelog_rows = Vec::new();
    let mut index_rows = Vec::new();
    let mut rule_ledger_rows_all = Vec::new();
    let mut upgrading_entries = Vec::new();
    for inc in &plan.increments {
        for adr in &inc.adrs {
            let path = decisions.join(format!("{number:04}-{}.md", adr.slug));
            // Refuse to clobber an ADR that already exists: a number that is not
            // actually free (e.g. debris from a prior partial run) must surface,
            // not be silently overwritten and the series restarted.
            if path.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "ADR {} already exists — refusing to overwrite",
                        path.display()
                    ),
                ));
            }
            adr_files.push((path, adr_file_contents(number, adr, inc.version)));
            index_rows.push(index_row(number, adr, inc.version));
            number += 1;
        }
        changelog_rows.push(changelog_row(inc.version, &inc.changelog));
        rule_ledger_rows_all.extend(rule_ledger_rows(inc, ledger_pr_number));
        if let Some(note) = &inc.breaking {
            upgrading_entries.push(crate::release_notes::upgrading_entry(inc.version, note));
        }
    }

    // Newest on top: the highest version/number is applied last, so reverse the
    // ascending lists before prepending them as a block.
    changelog_rows.reverse();
    index_rows.reverse();
    rule_ledger_rows_all.reverse();
    upgrading_entries.reverse();

    let changelog_before = fs::read_to_string(&changelog_path)?;
    let changelog_after =
        insert_after_table_separator(&changelog_before, "| Version |", &changelog_rows)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // Only touch the index when there are ADRs to add; `None` means "leave it".
    let readme_edit = if index_rows.is_empty() {
        None
    } else {
        let before = fs::read_to_string(&readme_path)?;
        let after = insert_after_table_separator(&before, "| # | Decision |", &index_rows)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Some((before, after))
    };

    // Only touch the ledger when some increment actually closes a rule.
    // Unlike the changelog/index, this file may not exist yet — no increment
    // has ever cited `closes_rule` before #1001 — so a missing file synthesises
    // a fresh header rather than erroring; `ledger_before` stays `None` in that
    // case so a rollback removes the file instead of restoring stale content.
    let ledger_existed = rule_ledger_path.exists();
    let ledger_edit = if rule_ledger_rows_all.is_empty() {
        None
    } else {
        let ledger_before = if ledger_existed {
            Some(fs::read_to_string(&rule_ledger_path)?)
        } else {
            None
        };
        let base = ledger_before
            .clone()
            .unwrap_or_else(|| RULE_LEDGER_HEADER.to_string());
        let after = insert_after_table_separator(&base, "| Rule |", &rule_ledger_rows_all)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Some((ledger_before, after))
    };

    // Only touch the upgrading page when some increment declared `breaking:` —
    // a tree whose increments break nothing never needs the page to exist, which
    // keeps the stamp backward-compatible with fixtures and forks that lack it.
    let upgrading_edit = if upgrading_entries.is_empty() {
        None
    } else {
        let before = fs::read_to_string(&upgrading_path)?;
        let after = crate::release_notes::insert_upgrading_entries(&before, &upgrading_entries)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Some((before, after))
    };

    // --- Commit: mutate the tree. Track what we write so a mid-flight failure
    // (the injected `bump` is the likely one) can be unwound. ---
    let mut written_adrs: Vec<PathBuf> = Vec::new();
    let commit = (|| -> io::Result<()> {
        for (path, contents) in &adr_files {
            fs::write(path, contents)?;
            written_adrs.push(path.clone());
        }
        fs::write(&changelog_path, &changelog_after)?;
        if let Some((_, after)) = &readme_edit {
            fs::write(&readme_path, after)?;
        }
        if let Some((_, after)) = &ledger_edit {
            fs::write(&rule_ledger_path, after)?;
        }
        if let Some((_, after)) = &upgrading_edit {
            fs::write(&upgrading_path, after)?;
        }
        // The flaky step, run last so its failure unwinds cleanly here rather
        // than stranding a changelog that cites a version the manifests never
        // reached.
        bump(plan.final_version())
    })();

    if let Err(e) = commit {
        // Roll back, best effort: restore the edited docs to their pre-apply
        // bytes and delete the ADR files we created. The pending files were not
        // touched (they are consumed only below), so the run is a clean retry.
        let _ = fs::write(&changelog_path, &changelog_before);
        if let Some((before, _)) = &readme_edit {
            let _ = fs::write(&readme_path, before);
        }
        if let Some((before, _)) = &upgrading_edit {
            let _ = fs::write(&upgrading_path, before);
        }
        if let Some((before, _)) = &ledger_edit {
            match before {
                Some(orig) => {
                    let _ = fs::write(&rule_ledger_path, orig);
                }
                None => {
                    let _ = fs::remove_file(&rule_ledger_path);
                }
            }
        }
        for path in &written_adrs {
            let _ = fs::remove_file(path);
        }
        return Err(e);
    }

    // Consume last: delete each pending file only once everything else has
    // succeeded, so a re-run finds nothing to do.
    for inc in &plan.increments {
        fs::remove_file(root.join("design/pending").join(&inc.file))?;
    }
    Ok(())
}

// --- Pure rendering / parsing helpers (unit-tested without the filesystem) ---

/// The `[workspace.package] version = "X.Y.Z"` from a `Cargo.toml`. Scoped to
/// the `[workspace.package]` section: the first line-anchored `version = "..."`
/// *within that table*, not a `version` under some other section (e.g. a stray
/// top-level `[package]`) nor the inline `version = "..."` inside a dependency
/// spec — the same value `bump-version.sh` rewrites.
pub fn parse_workspace_version(cargo_toml: &str) -> Option<Version> {
    let mut in_section = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        // A `[section]` header ends the previous table and opens a new one.
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == "[workspace.package]";
            continue;
        }
        if in_section
            && let Some(value) = line
                .strip_prefix("version = \"")
                .and_then(|r| r.strip_suffix('"'))
        {
            return Version::parse(value);
        }
    }
    None
}

/// The next free ADR number: one past the highest `NNNN-*.md` in
/// `design/decisions`. Errors if the directory can't be read — a swallowed
/// failure would silently default to `1` and restart the ADR series.
pub fn next_adr_number(root: &Path) -> io::Result<u32> {
    let dir = root.join("design/decisions");
    let mut max = 0;
    for entry in fs::read_dir(&dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let bytes = name.as_bytes();
        let is_adr = name.ends_with(".md")
            && bytes.len() > 5
            && bytes[4] == b'-'
            && bytes[..4].iter().all(u8::is_ascii_digit);
        if is_adr && let Ok(n) = name[..4].parse::<u32>() {
            max = max.max(n);
        }
    }
    Ok(max + 1)
}

/// The changelog table row for a stamped increment.
pub fn changelog_row(version: Version, blurb: &str) -> String {
    format!("| **v{version}** | {blurb} |")
}

/// The `design/decisions/NNNN-<slug>.md` file contents: a `# NNNN — <title>`
/// heading, a status line carrying the version, then the body verbatim.
pub fn adr_file_contents(number: u32, adr: &Adr, version: Version) -> String {
    format!(
        "# {number:04} — {title}\n\n- **Status:** {status} (v{ver})\n\n{body}\n",
        title = adr.title,
        status = adr.status(),
        ver = version.short(),
        body = adr.body,
    )
}

/// The `design/decisions/README.md` index row for a materialised ADR.
pub fn index_row(number: u32, adr: &Adr, version: Version) -> String {
    format!(
        "| [{number:04}]({number:04}-{slug}.md) | **{title}** (v{ver}) — {summary} | {status} (v{ver}) |",
        slug = adr.slug,
        title = adr.title,
        ver = version.short(),
        summary = adr.summary(),
        status = adr.status(),
    )
}

/// The header `design/greenfield-status-rules.md` is synthesised with the first
/// time any increment closes a rule — this file has no other reason to exist,
/// so `apply` doesn't require it pre-created the way it does the changelog and
/// the decisions index.
const RULE_LEDGER_HEADER: &str = "<!-- GENERATED, APPEND-ONLY — written by `cargo xtask stamp \
--apply` (xtask/src/stamp.rs) at merge, one row per rule id an increment cites in its \
`closes_rule` frontmatter (#1001).\n     Read by `cargo xtask greenfield-status` to populate \
the reference's rule-citation surface. Do not hand-edit. -->\n\n# Rules closed\n\n\
| Rule | Version | PR | Changelog |\n|---|---|---|---|\n";

/// One `design/greenfield-status-rules.md` row per rule id `inc` closes — empty
/// if `inc.closes_rule` is empty. `pr` renders as `#NNNN`, or the empty string
/// when it couldn't be recovered (kept as an empty cell, not a placeholder like
/// `"unknown"`, so the table stays a clean Markdown grid either way).
pub fn rule_ledger_rows(inc: &Stamped, pr: Option<u32>) -> Vec<String> {
    let pr_cell = pr.map(|n| format!("#{n}")).unwrap_or_default();
    inc.closes_rule
        .iter()
        .map(|rule| {
            format!(
                "| {rule} | v{} | {pr_cell} | {} |",
                inc.version, inc.changelog
            )
        })
        .collect()
}

/// Insert `rows` (already newest-first) immediately after the separator line of
/// the table whose header row contains `header_anchor` (e.g. `"| Version |"`).
/// Anchoring on the header — rather than the first separator in the file — keeps
/// the insert correct if another table (a legend, say) is ever added above.
pub fn insert_after_table_separator(
    md: &str,
    header_anchor: &str,
    rows: &[String],
) -> Result<String, String> {
    if rows.is_empty() {
        return Ok(md.to_string());
    }
    let mut out = Vec::new();
    let mut seen_header = false;
    let mut inserted = false;
    for line in md.lines() {
        out.push(line.to_string());
        if inserted {
            continue;
        }
        if !seen_header {
            seen_header = line.contains(header_anchor);
        } else if is_table_separator(line) {
            out.extend(rows.iter().cloned());
            inserted = true;
        }
    }
    if !inserted {
        return Err(format!(
            "no table with header {header_anchor:?} (and a `|---|` separator) to insert after"
        ));
    }
    // Preserve a trailing newline if the input had one.
    let mut joined = out.join("\n");
    if md.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

/// A Markdown table separator row — `|`, `-`, `:` and spaces only, with at least
/// one `-`.
fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adr(slug: &str, title: &str) -> Adr {
        Adr {
            slug: slug.into(),
            title: title.into(),
            summary: None,
            status: None,
            body: "**Decision.** Do the thing.".into(),
        }
    }

    #[test]
    fn version_next() {
        let v = Version {
            major: 0,
            minor: 185,
            patch: 0,
        };
        assert_eq!(v.next(&Level::Minor).to_string(), "0.186.0");
        assert_eq!(v.next(&Level::Patch).to_string(), "0.185.1");
        let p = Version {
            major: 0,
            minor: 185,
            patch: 3,
        };
        assert_eq!(p.next(&Level::Minor).to_string(), "0.186.0");
    }

    #[test]
    fn parse_workspace_version_ignores_dep_specs() {
        let cargo = "[workspace.package]\nversion = \"0.185.0\"\n\n[workspace.dependencies]\nbynk = { path = \"bynk\", version = \"0.185.0\" }\n";
        assert_eq!(
            parse_workspace_version(cargo).unwrap().to_string(),
            "0.185.0"
        );
    }

    #[test]
    fn parse_workspace_version_is_scoped_to_the_workspace_package_table() {
        // A line-anchored `version` outside `[workspace.package]` (here a stray
        // top-level `[package]`) must not be mistaken for the workspace version.
        let cargo = "[package]\nname = \"root\"\nversion = \"9.9.9\"\n\n[workspace.package]\nversion = \"0.185.0\"\n";
        assert_eq!(
            parse_workspace_version(cargo).unwrap().to_string(),
            "0.185.0"
        );
    }

    #[test]
    fn parse_workspace_version_none_without_the_table() {
        // A bare top-level `version` with no `[workspace.package]` table is not
        // the workspace version.
        assert!(parse_workspace_version("version = \"1.2.3\"\n").is_none());
    }

    #[test]
    fn changelog_row_format() {
        let v = Version {
            major: 0,
            minor: 186,
            patch: 0,
        };
        assert_eq!(
            changelog_row(v, "Add a thing"),
            "| **v0.186.0** | Add a thing |"
        );
    }

    #[test]
    fn version_short_follows_the_corpus_convention() {
        // Minor (patch == 0) → MAJOR.MINOR; patch → full.
        assert_eq!(
            Version {
                major: 0,
                minor: 186,
                patch: 0
            }
            .short(),
            "0.186"
        );
        assert_eq!(
            Version {
                major: 0,
                minor: 185,
                patch: 1
            }
            .short(),
            "0.185.1"
        );
    }

    #[test]
    fn adr_file_and_index_row_use_the_short_version() {
        let v = Version {
            major: 0,
            minor: 186,
            patch: 0,
        };
        let a = adr("a-slug", "The title");
        let file = adr_file_contents(206, &a, v);
        assert!(file.starts_with("# 0206 — The title\n"));
        // MINOR-only, matching every existing ADR — not the full 0.186.0.
        assert!(file.contains("- **Status:** Accepted (v0.186)"));
        assert!(file.trim_end().ends_with("Do the thing."));

        let row = index_row(206, &a, v);
        assert_eq!(
            row,
            "| [0206](0206-a-slug.md) | **The title** (v0.186) — The title | Accepted (v0.186) |"
        );
    }

    #[test]
    fn patch_increment_index_row_keeps_the_patch() {
        let v = Version {
            major: 0,
            minor: 185,
            patch: 1,
        };
        let a = adr("s", "T");
        assert!(index_row(9, &a, v).contains("(v0.185.1)"));
    }

    #[test]
    fn index_row_uses_explicit_summary_and_status() {
        let v = Version {
            major: 0,
            minor: 1,
            patch: 0,
        };
        let a = Adr {
            slug: "s".into(),
            title: "T".into(),
            summary: Some("the distillation".into()),
            status: Some("Proposed".into()),
            body: "b".into(),
        };
        assert_eq!(
            index_row(7, &a, v),
            "| [0007](0007-s.md) | **T** (v0.1) — the distillation | Proposed (v0.1) |"
        );
    }

    #[test]
    fn insert_prepends_after_the_anchored_table_newest_first() {
        let md =
            "## Recent increments\n\n| Version | Highlights |\n|---|---|\n| **v0.185.0** | Old |\n";
        let out = insert_after_table_separator(
            md,
            "| Version |",
            &["| NEW1 |".into(), "| NEW2 |".into()],
        )
        .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let sep = lines.iter().position(|l| *l == "|---|---|").unwrap();
        assert_eq!(lines[sep + 1], "| NEW1 |");
        assert_eq!(lines[sep + 2], "| NEW2 |");
        assert_eq!(lines[sep + 3], "| **v0.185.0** | Old |");
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn insert_skips_an_earlier_unrelated_table() {
        // A legend table sits above the target; the anchor must route past it.
        let md = "| Legend | Meaning |\n|---|---|\n| x | y |\n\n\
                  | Version | Highlights |\n|---|---|\n| **v0.185.0** | Old |\n";
        let out = insert_after_table_separator(md, "| Version |", &["| NEW |".into()]).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // Inserted below the *second* separator, not the legend's.
        let target = lines
            .iter()
            .position(|l| *l == "| **v0.185.0** | Old |")
            .unwrap();
        assert_eq!(lines[target - 1], "| NEW |");
        assert!(
            !out.contains("| x | y |\n| NEW |"),
            "must not touch the legend table"
        );
    }

    #[test]
    fn insert_without_the_anchored_table_errors() {
        assert!(
            insert_after_table_separator("no table here\n", "| Version |", &["| r |".into()])
                .is_err()
        );
    }
}
