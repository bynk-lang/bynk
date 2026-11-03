//! #2842: the driver's half of snapshot testing.
//!
//! The compiler inlines each suite file's stored snapshots into its test module
//! and the harness compares against them, reading nothing at test time. Only the
//! driver writes: under `--update-snapshots` the runner hands back, per snapshot
//! file, its counts and (when the file changes) the text to write, and
//! [`SnapshotRun::settle`] writes or deletes the files. A `snapshots/` file no
//! suite takes a snapshot into any more is obsolete as a whole; the build says
//! which files are live ([`bynk_emit::project::ProjectOutput::snapshot_files`]).

// A human-format run's report is one marked stdout line, taken out of the output;
// a line of test output that starts with the same marker is hidden too (the real
// report comes last and wins).
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The marker the human-format runner puts at the start of the one line that
/// carries the report (`BYNK_SNAPSHOT_REPORT=1`).
pub const REPORT_MARKER: char = '\u{1}';
const REPORT_PREFIX: &str = "bynk-snapshots ";

/// Environment: `1` makes the runner record rather than compare.
pub const UPDATE_ENV: &str = "BYNK_UPDATE_SNAPSHOTS";
/// Environment: `1` makes the human-format runner print the report line.
pub const REPORT_ENV: &str = "BYNK_SNAPSHOT_REPORT";

/// What a run did to the project's snapshots, as the report states it: in every
/// output format (`snapshots` in the JSON document and the NDJSON `run-end`).
/// `obsolete` counts snapshots no case takes any more — removed under
/// `--update-snapshots`, otherwise left and reported.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub written: u32,
    pub updated: u32,
    pub obsolete: u32,
}

/// One snapshot file's part of the runner's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub file: String,
    pub written: u32,
    pub updated: u32,
    pub obsolete: u32,
    /// Under the update flag, when the file changes: its new text, or `None` to
    /// delete it. Absent (`Absent`) when the file stays as it is.
    pub content: Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Absent,
    Write(String),
    Delete,
}

/// The runner's report: every selected module's snapshot files.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub files: Vec<FileReport>,
}

impl Report {
    /// The report in a runner event's `files` (an NDJSON `snapshots` event, or the
    /// human format's marked line).
    pub fn from_files_value(files: &serde_json::Value) -> Self {
        let num = |v: &serde_json::Value, key: &str| {
            v.get(key)
                .and_then(serde_json::Value::as_u64)
                .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
        };
        let files = files
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        Some(FileReport {
                            file: f.get("file")?.as_str()?.to_string(),
                            written: num(f, "written"),
                            updated: num(f, "updated"),
                            obsolete: num(f, "obsolete"),
                            content: match f.get("content") {
                                None => Change::Absent,
                                Some(serde_json::Value::Null) => Change::Delete,
                                Some(v) => Change::Write(v.as_str()?.to_string()),
                            },
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Report { files }
    }

    /// The report a human-format runner line carries, or `None` for any other
    /// line.
    pub fn from_marked_line(line: &str) -> Option<Self> {
        let rest = line
            .strip_prefix(REPORT_MARKER)?
            .strip_prefix(REPORT_PREFIX)?;
        let value: serde_json::Value = serde_json::from_str(rest.trim()).ok()?;
        Some(Self::from_files_value(&value))
    }
}

/// The project's snapshots for one run: which files the build takes snapshots
/// into, and what is stored.
#[derive(Debug, Clone)]
pub struct SnapshotRun {
    /// `--update-snapshots`.
    pub update: bool,
    root: PathBuf,
    live: BTreeSet<String>,
    stored: BTreeMap<String, String>,
}

impl SnapshotRun {
    /// `None` when the build takes no snapshot and nothing is stored: a project
    /// without snapshots is untouched and reports none.
    pub fn new(
        update: bool,
        root: &Path,
        live: BTreeSet<String>,
        stored: BTreeMap<String, String>,
    ) -> Option<Self> {
        let run = SnapshotRun {
            update,
            root: root.to_path_buf(),
            live,
            stored,
        };
        (!run.live.is_empty() || !run.orphans().is_empty()).then_some(run)
    }

    /// The stored files no suite takes a snapshot into, each with its snapshot
    /// count.
    pub fn orphans(&self) -> Vec<(String, u32)> {
        self.stored
            .iter()
            .filter(|(file, _)| !self.live.contains(*file))
            .map(|(file, text)| (file.clone(), entries_in(text)))
            .collect()
    }

    /// The report to settle: the runner's, or, when no module of the build takes a
    /// snapshot (so the runner reports none), an empty one — the stored files
    /// are all obsolete. `None` when a report was due and never came, as when the
    /// runner died: then nothing is written.
    pub fn report_for<'r>(&self, sent: Option<&'r Report>) -> Option<std::borrow::Cow<'r, Report>> {
        match sent {
            Some(report) => Some(std::borrow::Cow::Borrowed(report)),
            None if self.live.is_empty() => Some(std::borrow::Cow::Owned(Report::default())),
            None => None,
        }
    }

    /// The counts the run reports: the runner's, with each orphaned file's
    /// snapshots counted obsolete. `report` is `None` before the runner has said
    /// (the counts are then just the orphans').
    pub fn counts(&self, report: Option<&Report>) -> Counts {
        let mut counts = Counts::default();
        for f in report.map_or(&[][..], |r| r.files.as_slice()) {
            counts.written += f.written;
            counts.updated += f.updated;
            counts.obsolete += f.obsolete;
        }
        counts.obsolete += self.orphans().iter().map(|(_, n)| n).sum::<u32>();
        counts
    }

    /// Under `--update-snapshots`, write what the report says to write and
    /// delete what it says to delete, orphaned files included. Without the flag
    /// nothing changes. Returns the counts either way.
    pub fn settle(&self, report: &Report) -> std::io::Result<Counts> {
        if self.update {
            // Only the build's own snapshot files are written: the report may be
            // forged by test output on a human-format run.
            for f in report.files.iter().filter(|f| self.live.contains(&f.file)) {
                match &f.content {
                    Change::Absent => {}
                    Change::Write(text) => {
                        let path = self.root.join(&f.file);
                        if let Some(dir) = path.parent() {
                            std::fs::create_dir_all(dir)?;
                        }
                        std::fs::write(&path, text)?;
                    }
                    Change::Delete => self.remove(&f.file)?,
                }
            }
            for (file, _) in self.orphans() {
                self.remove(&file)?;
            }
        }
        Ok(self.counts(Some(report)))
    }

    /// Delete a snapshot file, then each directory it leaves empty, up to and
    /// including `snapshots/` itself.
    fn remove(&self, file: &str) -> std::io::Result<()> {
        let path = self.root.join(file);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let stop = self
            .root
            .join(bynk_project::snapshots::PROJECT_SNAPSHOTS_DIR);
        let mut dir = path.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if !d.starts_with(&stop) || std::fs::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        Ok(())
    }

    /// The line `rich` output ends with, or `None` when there is nothing to say.
    pub fn summary_line(&self, counts: Counts) -> Option<String> {
        let mut parts = Vec::new();
        if counts.written > 0 {
            parts.push(format!("{} written", counts.written));
        }
        if counts.updated > 0 {
            parts.push(format!("{} updated", counts.updated));
        }
        if counts.obsolete > 0 {
            parts.push(if self.update {
                format!("{} obsolete removed", counts.obsolete)
            } else {
                format!(
                    "{} obsolete (remove with --update-snapshots)",
                    counts.obsolete
                )
            });
        }
        (!parts.is_empty()).then(|| format!("snapshots: {}", parts.join(", ")))
    }
}

/// How many snapshots a stored file holds (its cases' entries). An unreadable
/// file counts as one, so it is still reported.
fn entries_in(text: &str) -> u32 {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(cases)) => cases
            .values()
            .map(|c| c.as_object().map_or(1, |o| o.len()))
            .sum::<usize>()
            .try_into()
            .unwrap_or(u32::MAX),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(update: bool, live: &[&str], stored: &[(&str, &str)]) -> Option<SnapshotRun> {
        SnapshotRun::new(
            update,
            Path::new("/nowhere"),
            live.iter().map(|s| s.to_string()).collect(),
            stored
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_project_without_snapshots_has_no_run() {
        assert!(run(false, &[], &[]).is_none());
    }

    #[test]
    fn an_unclaimed_file_is_obsolete_by_its_entries() {
        let r = run(
            false,
            &["snapshots/a.snap.json"],
            &[
                ("snapshots/a.snap.json", "{}"),
                (
                    "snapshots/b.snap.json",
                    r#"{"c":{"x":1,"y":2},"d":{"z":3}}"#,
                ),
            ],
        )
        .unwrap();
        assert_eq!(r.orphans(), vec![("snapshots/b.snap.json".to_string(), 3)]);
        assert_eq!(r.counts(None).obsolete, 3);
    }

    #[test]
    fn a_report_line_round_trips() {
        let line = "\u{1}bynk-snapshots [{\"file\":\"snapshots/a.snap.json\",\"written\":1,\"updated\":2,\"obsolete\":3,\"content\":\"{}\\n\"},{\"file\":\"snapshots/b.snap.json\",\"written\":0,\"updated\":0,\"obsolete\":1,\"content\":null}]";
        let report = Report::from_marked_line(line).unwrap();
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.files[0].content, Change::Write("{}\n".to_string()));
        assert_eq!(report.files[1].content, Change::Delete);
        assert!(Report::from_marked_line("  ✓ a case").is_none());
    }

    #[test]
    fn the_summary_says_what_changed() {
        let r = run(false, &["snapshots/a.snap.json"], &[]).unwrap();
        assert_eq!(r.summary_line(Counts::default()), None);
        assert_eq!(
            r.summary_line(Counts {
                written: 2,
                updated: 1,
                obsolete: 3
            })
            .as_deref(),
            Some("snapshots: 2 written, 1 updated, 3 obsolete (remove with --update-snapshots)")
        );
    }
}
