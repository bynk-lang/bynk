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
