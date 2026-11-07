//! #2842: snapshot testing, end to end through the real CLI. A scratch copy of
//! `fixtures/behaviour/snapshots` (a suite of three cases taking four
//! snapshots, none recorded) is driven through each state of a snapshot's life:
//! missing, recorded, matching, changed, updated, obsolete and removed.
//!
//! Like the other toolchain-driving tests it skips on Windows and, loudly, when
//! no TypeScript runner is on `PATH`; `BYNK_REQUIRE_TSC` makes the skip a failure.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../require/mod.rs"]
mod require;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";
const SNAP_FILE: &str = "snapshots/tests/shape.snap.json";

fn have_runner() -> bool {
    let has = |name: &str| {
        Command::new(if cfg!(windows) { "where" } else { "which" })
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    has("tsx") || (has("tsc") && has("node")) || has("npx")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/snapshots")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A fresh scratch copy of the fixture. The runner derives its `out-js` tree as
/// a sibling of `--output`, so each project gets its own parent.
fn project(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("snapshots-{name}"));
    let _ = fs::remove_dir_all(&root);
    copy_dir(&fixture(), &root.join("project"));
    root.join("project")
}

struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Run {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

fn test(project: &Path, args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(project)
        .arg("--output")
        .arg(project.parent().unwrap().join("out"))
        .args(args)
        .output()
        .expect("run bynk test");
    Run {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn snapshot(project: &Path) -> serde_json::Value {
    let text = fs::read_to_string(project.join(SNAP_FILE)).expect("the snapshot file exists");
    serde_json::from_str(&text).expect("the snapshot file is JSON")
}

fn rewrite(path: &Path, from: &str, to: &str) {
    let text = fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{from:?} not in {}", path.display());
    fs::write(path, text.replacen(from, to, 1)).unwrap();
}

/// Whether the toolchain is there; the test returns early when it is not.
fn ready() -> bool {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
        return false;
    }
    if !have_runner() {
        if require::is_required(REQUIRE_ENV) {
            panic!("no TypeScript runner on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return false;
    }
    true
}

#[test]
fn a_missing_snapshot_fails_and_the_flag_records_it() {
    if !ready() {
        return;
    }
    let p = project("missing");
    let missing = test(&p, &[]);
    assert!(!missing.ok, "a missing snapshot fails:\n{}", missing.all());
    assert!(
        missing.all().contains("no snapshot \"first\" is recorded"),
        "{}",
        missing.all()
    );
    assert!(!p.join(SNAP_FILE).exists(), "a plain run writes nothing");

    let written = test(&p, &["--update-snapshots"]);
    assert!(written.ok, "{}", written.all());
    assert!(
        written.stdout.contains("snapshots: 5 written"),
        "{}",
        written.stdout
    );
    let stored = snapshot(&p);
    assert_eq!(stored["a delivery reads"]["first"]["id"], "d-1");
    // Names are plain keys, even ones an object inherits.
    assert_eq!(stored["constructor"]["__proto__"]["id"], "d-4");
    assert_eq!(stored["two snapshots in one case"]["tags"][0], "b");
    // Sorted keys, two-space indent, a closing newline: reviewable text.
    let text = fs::read_to_string(p.join(SNAP_FILE)).unwrap();
    assert!(text.starts_with("{\n  \"a delivery reads\": {\n"), "{text}");
    assert!(text.ends_with("}\n"), "{text}");

    let again = test(&p, &[]);
    assert!(again.ok, "a recorded snapshot passes:\n{}", again.all());
    assert!(!again.all().contains("snapshots:"), "{}", again.all());
    let unchanged = test(&p, &["--update-snapshots"]);
    assert!(unchanged.ok && !unchanged.stdout.contains("snapshots:"));
}

#[test]
fn a_changed_value_fails_with_a_diff_and_the_flag_updates_it() {
    if !ready() {
        return;
    }
    let p = project("changed");
    assert!(test(&p, &["--update-snapshots"]).ok);
    rewrite(
        &p.join("src/shape.bynk"),
        "[\"b\", \"a\"]",
        "[\"b\", \"c\"]",
    );

    let failed = test(&p, &[]);
    assert!(!failed.ok, "{}", failed.all());
    let out = failed.all();
    assert!(out.contains("snapshot \"first\" differs"), "{out}");
    assert!(
        out.contains("-     \"a\""),
        "the stored line is marked -:\n{out}"
    );
    assert!(
        out.contains("+     \"c\""),
        "the new line is marked +:\n{out}"
    );
    assert_eq!(snapshot(&p)["a delivery reads"]["first"]["tags"][1], "a");

    let updated = test(&p, &["--update-snapshots"]);
    assert!(updated.ok, "{}", updated.all());
    assert!(
        updated.stdout.contains("snapshots: 5 updated"),
        "{}",
        updated.stdout
    );
    assert_eq!(snapshot(&p)["a delivery reads"]["first"]["tags"][1], "c");
    assert!(test(&p, &[]).ok);
}

#[test]
fn an_obsolete_snapshot_is_reported_and_the_flag_removes_it() {
    if !ready() {
        return;
    }
    let p = project("obsolete");
    assert!(test(&p, &["--update-snapshots"]).ok);
    // Retire a case and one of a case's two snapshots.
    let suite = p.join("tests/shape.bynk");
    let text = fs::read_to_string(&suite).unwrap();
    let cut = text.find("case \"a retired case\"").unwrap();
    fs::write(&suite, &text[..cut]).unwrap();
    rewrite(&suite, "\texpect d.tags matches snapshot \"tags\"\n", "");

    let reported = test(&p, &[]);
    assert!(
        reported.ok,
        "an obsolete snapshot does not fail:\n{}",
        reported.all()
    );
    assert!(
        reported
            .stdout
            .contains("snapshots: 2 obsolete (remove with --update-snapshots)"),
        "{}",
        reported.stdout
    );
    assert!(snapshot(&p).get("a retired case").is_some(), "still stored");

    let removed = test(&p, &["--update-snapshots"]);
    assert!(removed.ok, "{}", removed.all());
    assert!(
        removed.stdout.contains("snapshots: 2 obsolete removed"),
        "{}",
        removed.stdout
    );
    let stored = snapshot(&p);
    assert!(stored.get("a retired case").is_none());
    assert!(stored["two snapshots in one case"].get("tags").is_none());
    assert!(stored["two snapshots in one case"].get("whole").is_some());
    assert!(test(&p, &[]).ok);
}

#[test]
fn a_filtered_update_keeps_the_snapshots_of_cases_that_did_not_run() {
    if !ready() {
        return;
    }
    let p = project("filtered");
    assert!(test(&p, &["--update-snapshots"]).ok);
    rewrite(&p.join("src/shape.bynk"), "Closed(n)", "Closed(n + 1)");
    let run = test(&p, &["--update-snapshots", "--case", "a delivery reads"]);
    assert!(run.ok, "{}", run.all());
    assert!(
        run.stdout.contains("snapshots: 1 updated"),
        "{}",
        run.stdout
    );
    let stored = snapshot(&p);
    assert_eq!(stored["a delivery reads"]["first"]["kind"]["n"], 4);
    assert_eq!(
        stored["two snapshots in one case"]["whole"]["kind"]["n"], 4,
        "a case the filter skipped keeps its stored value"
    );
    assert!(
        stored.get("a retired case").is_some(),
        "and none is obsolete"
    );
}

#[test]
fn a_deleted_suite_leaves_an_obsolete_file() {
    if !ready() {
        return;
    }
    let p = project("deleted");
    assert!(test(&p, &["--update-snapshots"]).ok);
    fs::write(
        p.join("tests/shape.bynk"),
        "suite shape\n\ncase \"plain\" {\n\texpect read(\"a\", 1).id == \"a\"\n}\n",
    )
    .unwrap();
    let reported = test(&p, &[]);
    assert!(reported.ok, "{}", reported.all());
    assert!(
        reported.stdout.contains("snapshots: 5 obsolete"),
        "{}",
        reported.stdout
    );
    let removed = test(&p, &["--update-snapshots"]);
    assert!(removed.ok, "{}", removed.all());
    assert!(!p.join(SNAP_FILE).exists(), "the file is deleted");
    assert!(!p.join("snapshots").exists(), "and so is its empty folder");
}

#[test]
fn json_and_ndjson_carry_the_counts() {
    if !ready() {
        return;
    }
    let p = project("formats");
    let first = test(&p, &["--format", "json", "--update-snapshots"]);
    let doc: serde_json::Value = serde_json::from_str(&first.stdout).expect("a document");
    assert_eq!(
        doc["snapshots"],
        serde_json::json!({"written": 5, "updated": 0, "obsolete": 0})
    );
    let again = test(&p, &["--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&again.stdout).expect("a document");
    assert_eq!(doc["failed"], 0);
    assert_eq!(
        doc["snapshots"],
        serde_json::json!({"written": 0, "updated": 0, "obsolete": 0})
    );

    rewrite(
        &p.join("src/shape.bynk"),
        "[\"b\", \"a\"]",
        "[\"b\", \"c\"]",
    );
    let stream = test(&p, &["--format", "ndjson", "--update-snapshots"]);
    let last = stream.stdout.lines().last().expect("events");
    let end: serde_json::Value = serde_json::from_str(last).expect("a run-end");
    assert_eq!(end["type"], "run-end");
    assert_eq!(
        end["snapshots"],
        serde_json::json!({"written": 0, "updated": 5, "obsolete": 0})
    );
}

#[test]
fn update_snapshots_is_refused_where_it_cannot_record() {
    let p = project("refused");
    for flag in ["--against", "--coverage", "--inspect", "--no-run"] {
        let args: Vec<&str> = if flag == "--against" {
            vec!["--update-snapshots", "--against", "dev"]
        } else {
            vec!["--update-snapshots", flag]
        };
        let run = test(&p, &args);
        assert!(!run.ok, "{flag} is refused");
        assert!(
            run.stderr.contains(&format!(
                "`--update-snapshots` cannot be combined with `{flag}`"
            )),
            "{flag}: {}",
            run.stderr
        );
    }
    assert!(!p.join(SNAP_FILE).exists());
}

#[test]
fn a_corrupt_file_fails_and_a_filtered_update_leaves_it_alone() {
    if !ready() {
        return;
    }
    let p = project("corrupt");
    fs::create_dir_all(p.join("snapshots/tests")).unwrap();
    fs::write(p.join(SNAP_FILE), "not json").unwrap();
    let failed = test(&p, &[]);
    assert!(
        !failed.ok && failed.all().contains("not a JSON object"),
        "{}",
        failed.all()
    );
    let filtered = test(&p, &["--update-snapshots", "--case", "a delivery reads"]);
    assert!(filtered.ok, "{}", filtered.all());
    assert!(
        !filtered.stdout.contains("written"),
        "nothing was written:\n{}",
        filtered.stdout
    );
    assert_eq!(fs::read_to_string(p.join(SNAP_FILE)).unwrap(), "not json");
    assert!(test(&p, &["--update-snapshots"]).ok);
    assert!(snapshot(&p).get("a delivery reads").is_some());
}
