//! #3069 behavioural tests for `bynk test --mutate`: a surviving mutant is
//! reported with its span, a killed one is not, a unit no suite reaches is
//! `no suite`, the working tree is never written, and a failing baseline or an
//! incompatible flag refuses the run. Drives the real `bynk test` CLI against a
//! small fixture.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).

use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `false` (after saying why) when the test cannot run here. `--mutate` needs
/// the `tsc → node` path as well as the in-process one.
fn can_run() -> bool {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
        return false;
    }
    if !(tool_exists("tsc") && tool_exists("node")) {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no tsc+node on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no tsc+node on PATH");
        return false;
    }
    true
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/mutate")
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

fn run_in(project: &Path, args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(project)
        .args(args)
        .output()
        .expect("run bynk test");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        success: out.status.success(),
    }
}

fn mutants(doc: &serde_json::Value) -> Vec<(String, String, String)> {
    doc["mutation"]["mutants"]
        .as_array()
        .expect("mutants")
        .iter()
        .map(|m| {
            (
                m["decl"].as_str().unwrap().to_string(),
                m["to"].as_str().unwrap().to_string(),
                m["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_weak_suite_leaves_a_survivor_with_its_span_and_the_tree_is_untouched() {
    if !can_run() {
        return;
    }
    let before = std::fs::read_to_string(fixture().join("src/demo/limits.bynk")).unwrap();
    let r = run_in(&fixture(), &["--mutate", "--format", "json"]);
    assert!(
        r.success,
        "survivors alone do not fail the run: {}",
        r.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("a JSON document: {e}\n{}\n{}", r.stdout, r.stderr));
    assert_eq!(doc["failed"], 0, "the baseline's own result is kept");
    assert_eq!(
        mutants(&doc),
        vec![
            ("within".into(), "<=".into(), "survived".into()),
            ("positive".into(), ">=".into(), "killed".into()),
            ("same".into(), "!=".into(), "no_suite".into()),
        ]
    );
    let m = &doc["mutation"];
    assert_eq!(m["killed"], 1);
    assert_eq!(m["survived"], 1);
    assert_eq!(m["no_suite"], 1);
    assert_eq!(m["score"], 50);
    let survivor = &m["mutants"][0];
    assert_eq!(survivor["file"], "src/demo/limits.bynk");
    assert_eq!(survivor["line"], 4);
    assert_eq!(survivor["column"], 5);
    assert_eq!(survivor["from"], "<");

    let after = std::fs::read_to_string(fixture().join("src/demo/limits.bynk")).unwrap();
    assert_eq!(before, after, "a mutant is never written to the project");
    assert!(
        !fixture().join("out").exists(),
        "the scratch output is not the project's `out/`"
    );
}

#[test]
fn rich_output_names_the_survivor_and_the_gap() {
    if !can_run() {
        return;
    }
    let r = run_in(&fixture(), &["--mutate"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout
            .contains("SURVIVED  src/demo/limits.bynk:4:5  demo.limits.within  `<` → `<=`"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("NO SUITE"), "{}", r.stdout);
    assert!(
        !r.stdout.contains("positive"),
        "a killed mutant is not listed"
    );
}

#[test]
fn only_selects_one_declaration_and_fail_under_gates_the_exit_code() {
    if !can_run() {
        return;
    }
    let r = run_in(
        &fixture(),
        &[
            "--mutate",
            "--only",
            "demo.limits.within",
            "--format",
            "json",
        ],
    );
    assert!(r.success, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(
        mutants(&doc),
        vec![("within".into(), "<=".into(), "survived".into())]
    );

    let r = run_in(
        &fixture(),
        &[
            "--mutate",
            "--only",
            "demo.limits",
            "--mutate-fail-under",
            "100",
        ],
    );
    assert!(!r.success, "a score of 50% is under 100");
    assert!(
        r.stderr.contains("below --mutate-fail-under"),
        "{}",
        r.stderr
    );

    let r = run_in(&fixture(), &["--mutate", "--only", "demo.nothing"]);
    assert!(!r.success);
    assert!(r.stderr.contains("names no declaration"), "{}", r.stderr);
}

#[test]
fn incompatible_flags_are_refused() {
    let r = run_in(&fixture(), &["--mutate", "--coverage"]);
    assert!(!r.success);
    assert!(
        r.stderr
            .contains("`--mutate` cannot be combined with `--coverage`"),
        "{}",
        r.stderr
    );
    let r = run_in(&fixture(), &["--only", "demo.limits"]);
    assert!(!r.success, "`--only` needs `--mutate`");
}

#[test]
fn a_failing_baseline_refuses_to_mutate() {
    if !can_run() {
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mutate-red-baseline");
    let _ = std::fs::remove_dir_all(&dir);
    for rel in [
        "src/demo/limits.bynk",
        "src/demo/lonely.bynk",
        "src/tests/limits.test.bynk",
    ] {
        let to = dir.join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        let text = std::fs::read_to_string(fixture().join(rel)).unwrap();
        // `within(1, 5)` is true; claim it is not.
        std::fs::write(
            to,
            text.replace("within(1, 5) == true", "within(1, 5) == false"),
        )
        .unwrap();
    }
    let r = run_in(&dir, &["--mutate"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("the unmutated project does not pass"),
        "{}",
        r.stderr
    );
}

fn copy_fixture(name: &str, manifest: Option<&str>) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for rel in [
        "src/demo/limits.bynk",
        "src/demo/lonely.bynk",
        "src/tests/limits.test.bynk",
    ] {
        let to = dir.join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(fixture().join(rel), to).unwrap();
    }
    if let Some(manifest) = manifest {
        std::fs::write(dir.join("bynk.toml"), manifest).unwrap();
    }
    dir
}

#[test]
fn test_mutate_exclude_leaves_matching_code_unmutated() {
    if !can_run() {
        return;
    }
    let dir = copy_fixture(
        "mutate-exclude",
        Some("[test.mutate]\nexclude = [\"src/demo/lonely.bynk\", \"demo.limits.positive\"]\n"),
    );
    let r = run_in(&dir, &["--mutate", "--format", "json"]);
    assert!(r.success, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(
        mutants(&doc),
        vec![("within".into(), "<=".into(), "survived".into())],
        "a path glob and a declaration name each drop their mutants"
    );
}

#[test]
fn a_run_that_outlives_the_timeout_is_a_timeout_not_invalid() {
    if !can_run() {
        return;
    }
    // A zero timeout kills every mutant's run at once, whatever it was doing.
    let r = run_in(
        &fixture(),
        &["--mutate", "--mutate-timeout", "0", "--format", "json"],
    );
    assert!(r.success, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    let statuses: Vec<String> = mutants(&doc).into_iter().map(|m| m.2).collect();
    assert_eq!(statuses, vec!["timeout", "timeout", "no_suite"]);
    assert_eq!(doc["mutation"]["timeout"], 2);
    assert_eq!(doc["mutation"]["invalid"], 0);
}

#[test]
fn guard_and_arithmetic_mutants_are_reported_and_a_one_sided_suite_leaves_a_survivor() {
    if !can_run() {
        return;
    }
    let project = fixture().with_file_name("mutate_guards");
    let r = run_in(&project, &["--mutate", "--format", "json"]);
    assert!(r.success, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("a JSON document: {e}\n{}\n{}", r.stdout, r.stderr));
    let status = |decl: &str, from: &str, to: &str| -> String {
        doc["mutation"]["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["decl"] == decl && m["from"] == from && m["to"] == to)
            .unwrap_or_else(|| panic!("no mutant {decl} {from} -> {to}: {}", r.stdout))["status"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // The suite only tries `n == 10`, where the `if` is already true.
    assert_eq!(status("bump", "n == 10", "true"), "survived");
    assert_eq!(status("bump", "n == 10", "false"), "killed");
    assert_eq!(status("bump", "+", "-"), "killed");
    // Both sides of the arm's guard are tried.
    assert_eq!(status("small", "n == 3", "true"), "killed");
    assert_eq!(status("small", "n == 3", "false"), "killed");
}

#[test]
fn sql_mutants_are_reported_and_a_boundary_free_suite_leaves_a_survivor() {
    if !can_run() {
        return;
    }
    let project = fixture().with_file_name("mutate_sql");
    let r = run_in(&project, &["--mutate", "--format", "json"]);
    assert!(r.success, "{}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("a JSON document: {e}\n{}\n{}", r.stdout, r.stderr));
    let status = |from: &str, to: &str| -> String {
        doc["mutation"]["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["decl"] == "above" && m["from"] == from && m["to"] == to)
            .unwrap_or_else(|| panic!("no mutant {from} -> {to}: {}", r.stdout))["status"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // The suite never asks at the boundary, so the weaker comparison survives.
    assert_eq!(status(">", ">="), "survived");
    // Dropping either conjunct changes the count the suite checks.
    assert_eq!(status("AND live = 1", ""), "killed");
    assert_eq!(status("n > ?1 AND", ""), "killed");
}
