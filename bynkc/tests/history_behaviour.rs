//! v0.119 behavioural test for history properties (testing track slice 7, ADR
//! 0155). Snapshots prove the emitted shape; this proves the runtime behaviour: a
//! `for all run: History[Agent]` property that fails over its generated call-
//! histories drives the real handlers, reports a run count, the run's root seed,
//! and a shrunk **sequence** with a copy-paste reproduce line — and `--seed <hex>`
//! reproduces the run byte-for-byte.
//!
//! Drives the real `bynkc test` CLI against a fixture project, so it exercises the
//! `--seed` threading, the driver, the runner, and sequence shrinking end to end.
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner (`tsc`+`node` or `tsx`) is available; `BYNK_REQUIRE_TSC=1` turns the skip
//! into a failure (CI).

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

/// A TypeScript runner is available if either `tsx` or (`tsc` + `node`) is on
/// PATH — the same fallback chain `bynkc test` itself walks.
fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/history_fail")
}

/// Run `bynkc test <fixture> --seed <seed>` and return combined stdout+stderr.
fn run_with_seed(seed: &str, out_dir: &str) -> String {
    // Each run gets its own parent dir: the runner derives the executed
    // `out-js` tree as a *sibling* of `--output`, so a shared parent (e.g. two
    // behavioural tests both rooted directly under `CARGO_TARGET_TMPDIR`)
    // would race the emitted `.js` a concurrent test's `tsc`/`node` run reads.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .arg("--seed")
        .arg(seed)
        .output()
        .expect("run bynkc test");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[test]
fn failing_history_property_reports_seed_and_shrunk_sequence() {
    // `bynkc test`'s runner detection is Unix-only (it shells out to `which`), so
    // the CLI cannot locate a runner on Windows and never runs the emitted tests.
    // The emission is covered platform-independently by `248_history_property`.
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    let first = run_with_seed("0x5f3a", "history-a");

    // A failure over the generated call-histories reports the run count, the run's
    // root seed (the one we passed), a shrunk *sequence*, and a reproduce line.
    assert!(
        first.contains("history property failed after"),
        "expected a history-failure banner, got:\n{first}"
    );
    assert!(
        first.contains("(seed 0x5f3a)"),
        "expected the passed root seed in the report, got:\n{first}"
    );
    assert!(
        first.contains("shrunk sequence:"),
        "expected a shrunk sequence, got:\n{first}"
    );
    assert!(
        first.contains("--seed 0x5f3a"),
        "expected a reproduce line carrying the root seed, got:\n{first}"
    );

    // A driven run deliberately provokes rejected steps, each of which would
    // otherwise `console.error` an `InvariantViolation` line from the agent's
    // `commitState`. The driver mutes exactly those lines for the duration of the
    // drive, so a rejection-heavy failing run stays readable.
    assert!(
        !first.contains("InvariantViolation"),
        "the driver must mute the agent's InvariantViolation logging, got:\n{first}"
    );

    // Re-running with the same seed reproduces the same shrunk sequence — the whole
    // point of a deterministic, seed-derived generator + driver.
    let second = run_with_seed("0x5f3a", "history-b");
    let extract = |s: &str| {
        s.lines()
            .find(|l| l.contains("shrunk sequence:"))
            .map(|l| l.trim().to_string())
    };
    assert_eq!(
        extract(&first),
        extract(&second),
        "the same seed must reproduce the same shrunk sequence"
    );
}

/// Track C (#2266): a failed history property reports `kind: "history"`, the root
/// seed and the shrunk sequence as data, alongside the unchanged message.
#[test]
fn a_failed_history_property_reports_its_seed_and_sequence_as_data() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("out-history-data")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynkc test");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("a JSON document");
    let failed = doc["suites"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["cases"].as_array().unwrap())
        .find(|c| c["outcome"] == "fail")
        .unwrap_or_else(|| panic!("a failed history property in {doc}"));
    assert_eq!(failed["kind"], "history");
    assert_eq!(failed["seed"], "0x5f3a");
    let sequence = failed["counterexample"]
        .as_str()
        .expect("a shrunk sequence");
    assert!(
        sequence.starts_with('[') && sequence.ends_with(']'),
        "a sequence: {sequence}"
    );
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .contains(&format!("shrunk sequence:  {sequence}")),
        "the data is the text's own sequence: {failed}"
    );
}
