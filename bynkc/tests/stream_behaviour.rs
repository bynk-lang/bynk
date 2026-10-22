//! #2267 (Slice D stage 1) behavioural tests for `bynkc test --format ndjson`:
//! the supported result stream. Drives the real CLI against the `suite_filter`
//! fixture (two suites that share a case name) and a project that does not
//! compile.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).

use std::path::PathBuf;
use std::process::{Command, Output};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn can_run() -> bool {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return false;
    }
    if !(tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")) {
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

/// `bynkc test <project> --format <format> <args…>`, each run in its own output
/// parent: the runner derives the executed tree as a sibling of `--output`.
fn test(
    project: &PathBuf,
    out_dir: &str,
    format: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Output {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynkc"));
    cmd.arg("test")
        .arg(project)
        .arg("--output")
        .arg(&out_root)
        .args(["--format", format])
        .args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run bynkc test")
}

/// The stream's events, each parsed from one line of stdout.
fn events(out: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| {
                panic!(
                    "every line is a JSON event: {e}\nline: {l}\nstderr: {}",
                    String::from_utf8_lossy(&out.stderr)
                )
            })
        })
        .collect()
}

fn types(events: &[serde_json::Value]) -> Vec<&str> {
    events
        .iter()
        .map(|e| e["type"].as_str().expect("a type"))
        .collect()
}

#[test]
fn a_run_streams_its_events_in_order_and_ends_with_run_end() {
    if !can_run() {
        return;
    }
    let out = test(&fixture(), "stream-order", "ndjson", &[], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let events = events(&out);
    assert_eq!(
        types(&events),
        [
            "run-begin",
            "suite-begin",
            "case",
            "case",
            "case",
            "suite-end",
            "suite-begin",
            "case",
            "case",
            "suite-end",
            "run-end"
        ]
    );
    assert_eq!(events[0]["suites"], 2);
    assert_eq!(events[1]["name"], "demo.a");
    assert_eq!(events[1]["kind"], "unit");
    assert_eq!(events[1]["tests"], 3);
    // A case event is the document's case behind `type` and `suite`.
    assert_eq!(events[2]["suite"], "demo.a");
    assert_eq!(events[2]["name"], "one");
    assert_eq!(events[2]["outcome"], "pass");
    assert_eq!(events[2]["tier"], "unit");
    assert_eq!(events.last().unwrap()["passed"], 5);
    assert_eq!(events.last().unwrap()["failed"], 0);
}

/// The stream and the document are two views of one run: the same cases, with
/// the same outcomes and tiers, in the same suites.
#[test]
fn the_stream_agrees_with_the_json_document() {
    if !can_run() {
        return;
    }
    let doc: serde_json::Value =
        serde_json::from_slice(&test(&fixture(), "stream-doc", "json", &[], &[]).stdout)
            .expect("a document");
    let stream = events(&test(&fixture(), "stream-agree", "ndjson", &[], &[]));

    let mut from_doc = Vec::new();
    for suite in doc["suites"].as_array().unwrap() {
        for case in suite["cases"].as_array().unwrap() {
            from_doc.push((
                suite["name"].as_str().unwrap().to_string(),
                case["name"].as_str().unwrap().to_string(),
                case["outcome"].as_str().unwrap().to_string(),
                case["tier"].as_str().map(str::to_string),
            ));
        }
    }
    let from_stream: Vec<_> = stream
        .iter()
        .filter(|e| e["type"] == "case")
        .map(|e| {
            (
                e["suite"].as_str().unwrap().to_string(),
                e["name"].as_str().unwrap().to_string(),
                e["outcome"].as_str().unwrap().to_string(),
                e["tier"].as_str().map(str::to_string),
            )
        })
        .collect();
    assert_eq!(from_stream, from_doc);
    assert_eq!(stream.last().unwrap()["passed"], doc["passed"]);
}

#[test]
fn a_filtered_run_still_ends_with_run_end() {
    if !can_run() {
        return;
    }
    let out = test(
        &fixture(),
        "stream-filter",
        "ndjson",
        &["--suite", "demo.b"],
        &[],
    );
    assert!(out.status.success());
    let events = events(&out);
    assert_eq!(
        types(&events),
        [
            "run-begin",
            "suite-begin",
            "case",
            "case",
            "suite-end",
            "run-end"
        ]
    );
    assert_eq!(events[0]["suites"], 1, "only the selected suite is counted");
}

#[test]
fn a_failing_run_exits_non_zero_after_its_run_end() {
    if !can_run() {
        return;
    }
    let project =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/property_fail");
    let out = test(&project, "stream-fail", "ndjson", &[], &[]);
    assert!(!out.status.success());
    let events = events(&out);
    let last = events.last().unwrap();
    assert_eq!(last["type"], "run-end");
    assert!(last["failed"].as_u64().unwrap() > 0);
    let failed = events
        .iter()
        .find(|e| e["type"] == "case" && e["outcome"] == "fail")
        .expect("a failed case");
    assert_eq!(
        failed["kind"], "property",
        "the generative fields ride the case"
    );
    assert!(failed["seed"].as_str().unwrap().starts_with("0x"));
}

#[test]
fn a_project_that_does_not_compile_is_one_error_event() {
    if !can_run() {
        return;
    }
    let project = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("stream-compile-error");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(project.join("src/demo")).unwrap();
    std::fs::create_dir_all(project.join("src/tests")).unwrap();
    std::fs::write(
        project.join("src/demo/m.bynk"),
        "commons demo.m\n\nfn id(n: Int) -> Int {\n  n + true\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.join("src/tests/m.test.bynk"),
        "suite demo.m {\n  case \"one\" {\n    expect id(1) == 1\n  }\n}\n",
    )
    .unwrap();
    let out = test(&project, "stream-compile-error-out", "ndjson", &[], &[]);
    assert!(!out.status.success());
    let events = events(&out);
    assert_eq!(types(&events), ["error"]);
    assert_eq!(events[0]["kind"], "compile");
    assert!(!events[0]["diagnostics"].as_array().unwrap().is_empty());
}

/// A runner that dies before `run-end` leaves the stream ending with an `error`,
/// whatever it managed to emit first. A filter the entry cannot parse crashes it
/// at load, so nothing is emitted before the error.
#[test]
fn a_runner_that_dies_ends_the_stream_with_an_error_event() {
    if !can_run() {
        return;
    }
    let out = test(
        &fixture(),
        "stream-crash",
        "ndjson",
        &[],
        &[("BYNK_TEST_FILTER", "this is not json")],
    );
    assert!(!out.status.success());
    let events = events(&out);
    assert_eq!(types(&events), ["error"]);
    assert_eq!(events[0]["kind"], "runtime");
    assert_eq!(
        events[0]["message"],
        "the test runner exited before completing"
    );
    assert!(events[0]["stderr"].as_str().is_some_and(|s| !s.is_empty()));
}

#[test]
fn ndjson_refuses_coverage_and_no_run() {
    for (flag, mention) in [("--coverage", "--coverage"), ("--no-run", "--no-run")] {
        let out = test(&fixture(), "stream-refuse", "ndjson", &[flag], &[]);
        assert!(!out.status.success(), "{flag}");
        let events = events(&out);
        assert_eq!(types(&events), ["error"], "{flag}");
        assert!(
            events[0]["message"].as_str().unwrap().contains(mention),
            "{flag}: {}",
            events[0]
        );
    }
}
