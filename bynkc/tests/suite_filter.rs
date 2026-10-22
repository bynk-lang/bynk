//! #2273 behavioural tests for `bynkc test --case` (repeatable) and `--suite`:
//! one process runs a chosen set of tests, a module stays one suite record, the
//! two flags combine as "and" and their values as "or", and a value that selects
//! nothing fails the run. Drives the real `bynkc test` CLI against a fixture of
//! two suites that share one case name.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).

use std::path::PathBuf;
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

fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

/// `false` (after saying why) when the test cannot run here.
fn can_run() -> bool {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return false;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return false;
    }
    true
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/suite_filter")
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Run {
    fn doc(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "a JSON document: {e}\nstdout:\n{}\nstderr:\n{}",
                self.stdout, self.stderr
            )
        })
    }
}

/// `bynkc test <fixture> <args…>`, each run with its own output parent: the
/// runner derives the executed tree as a sibling of `--output`.
fn run(out_dir: &str, args: &[&str]) -> Run {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .args(args)
        .output()
        .expect("run bynkc test");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        success: out.status.success(),
    }
}

fn json(out_dir: &str, args: &[&str]) -> Run {
    let mut all = vec!["--format", "json"];
    all.extend_from_slice(args);
    run(out_dir, &all)
}

/// The document's suites as `(name, case names)`, in order.
fn shape(doc: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    doc["suites"]
        .as_array()
        .expect("suites")
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().to_string(),
                s["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["name"].as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_repeated_case_runs_the_set_in_one_process_as_one_suite_record() {
    if !can_run() {
        return;
    }
    let r = json("sf-repeat", &["--case", "one", "--case", "three"]);
    assert!(r.success, "the set runs: {}", r.stderr);
    let doc = r.doc();
    assert_eq!(
        shape(&doc),
        vec![
            ("demo.a".to_string(), names(&["one"])),
            ("demo.b".to_string(), names(&["three"]))
        ]
    );
    assert_eq!(doc["passed"], 2, "each selected case ran once");

    // Two cases of one module are one record, in the order the flags gave them.
    let r = json("sf-order", &["--case", "two", "--case", "one"]);
    let doc = r.doc();
    let suites = shape(&doc);
    assert_eq!(suites[0], ("demo.a".to_string(), names(&["two", "one"])));
    assert_eq!(doc["passed"], 2);
}

#[test]
fn a_case_name_shared_by_two_suites_runs_in_both() {
    if !can_run() {
        return;
    }
    let r = json("sf-shared", &["--case", "shared"]);
    assert!(r.success, "{}", r.stderr);
    let doc = r.doc();
    assert_eq!(doc["passed"], 2);
    assert_eq!(
        shape(&doc),
        vec![
            ("demo.a".to_string(), names(&["shared"])),
            ("demo.b".to_string(), names(&["shared"]))
        ]
    );
}

#[test]
fn a_suite_selects_all_its_cases_and_omits_the_others() {
    if !can_run() {
        return;
    }
    let r = json("sf-suite", &["--suite", "demo.b"]);
    assert!(r.success, "{}", r.stderr);
    let doc = r.doc();
    assert_eq!(
        shape(&doc),
        vec![("demo.b".to_string(), names(&["three", "shared"]))],
        "the suite that was not selected is left out, not kept empty"
    );
    assert_eq!(doc["passed"], 2);

    let both = json("sf-suites", &["--suite", "demo.a", "--suite", "demo.b"]);
    assert_eq!(both.doc()["passed"], 5);
}

#[test]
fn suite_and_case_combine_as_and() {
    if !can_run() {
        return;
    }
    let r = json("sf-and", &["--suite", "demo.b", "--case", "shared"]);
    assert!(r.success, "{}", r.stderr);
    let doc = r.doc();
    assert_eq!(
        shape(&doc),
        vec![("demo.b".to_string(), names(&["shared"]))]
    );
    assert_eq!(doc["passed"], 1);

    // `one` is not in demo.b, so within it that value selects nothing.
    let r = json("sf-and-miss", &["--suite", "demo.b", "--case", "one"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("no case matched --case \"one\""),
        "{}",
        r.stderr
    );
}

#[test]
fn a_value_that_selects_nothing_fails_the_run_after_its_output() {
    if !can_run() {
        return;
    }
    // `--case one --case typo` fails although `one` matches: each value counts.
    let r = json("sf-typo", &["--case", "one", "--case", "typo"]);
    assert!(!r.success, "a typo is not a vacuous pass");
    assert_eq!(r.doc()["passed"], 1, "the usual document is still printed");
    assert!(
        r.stderr
            .contains("bynkc test: no case matched --case \"typo\""),
        "{}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("\"one\""),
        "only the unmatched value is named"
    );

    let r = json("sf-nosuite", &["--suite", "nope"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("no suite named --suite \"nope\""),
        "{}",
        r.stderr
    );

    // The same in the rich format.
    let r = run("sf-typo-rich", &["--case", "one", "--case", "typo"]);
    assert!(!r.success);
    assert!(r.stdout.contains("1 passed, 0 failed."), "{}", r.stdout);
    assert!(
        r.stderr
            .contains("bynkc test: no case matched --case \"typo\""),
        "{}",
        r.stderr
    );
}

#[test]
fn empty_and_duplicate_values_are_dropped() {
    if !can_run() {
        return;
    }
    let r = json("sf-dup", &["--case", "one", "--case", "one", "--case", ""]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(r.doc()["passed"], 1, "a duplicate runs once");

    let r = json("sf-empty", &["--case", ""]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(r.doc()["passed"], 5, "an empty --case is unset");
}

#[test]
fn coverage_over_a_set_is_one_report() {
    if !(can_run() && tool_exists("tsc") && tool_exists("node")) {
        eprintln!("skipping: coverage needs tsc and node");
        return;
    }
    let r = json(
        "sf-cov",
        &["--coverage", "--case", "one", "--case", "three"],
    );
    assert!(r.success, "{}", r.stderr);
    let doc = r.doc();
    assert_eq!(doc["passed"], 2);
    assert!(doc["coverage"].is_object(), "one coverage block: {doc}");
}

/// #2273: a case the build skipped (`--against … --skip-unsupported`) is not in
/// the build but exists, so a `--case` naming one is not unmatched. The driver
/// tells the entry which skipped cases the filter selects; this hands the entry
/// that filter directly.
#[test]
fn a_selected_skipped_case_is_not_unmatched() {
    if !can_run() {
        return;
    }
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sf-skipped")
        .join("out");
    let with = |filter: &str| {
        Command::new(env!("CARGO_BIN_EXE_bynkc"))
            .arg("test")
            .arg(fixture())
            .arg("--output")
            .arg(&out_root)
            .env("BYNK_TEST_FILTER", filter)
            .output()
            .expect("run bynkc test")
    };

    let told = with(r#"{"cases":["ghost"],"skipped":[{"suite":"demo.a","case":"ghost"}]}"#);
    assert!(
        told.status.success(),
        "a skipped case the filter selects exists: {}",
        String::from_utf8_lossy(&told.stderr)
    );

    let untold = with(r#"{"cases":["ghost"]}"#);
    assert!(
        !untold.status.success(),
        "without it the value selects nothing"
    );

    // A skipped case in a suite the filter leaves out does not rescue its name.
    let elsewhere = with(
        r#"{"suites":["demo.b"],"cases":["ghost"],"skipped":[{"suite":"demo.a","case":"ghost"}]}"#,
    );
    assert!(!elsewhere.status.success());
}
