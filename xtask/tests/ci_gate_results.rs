//! #3758: `CI green` and `CI fast gates` go green only when every needed job
//! reports `success` or `skipped`. They used to reject just `failure` and
//! `cancelled`. GitHub reports a matrix job whose leg never got a runner as
//! `abandoned`, so the nightlies of 19 and 20 November 2026 lost their macOS
//! test leg, `CI green` passed, and the nightly report never fired.
//!
//! These tests take each gate's script out of ci.yml as written, put a list
//! of results where `${{ join(needs.*.result, ' ') }}` stands, and run it.
// The scripts run under bash on the Linux runners. Windows has no bash on a
// plain developer machine.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

const RESULTS_EXPR: &str = "${{ join(needs.*.result, ' ') }}";

/// The gate steps, by `name:`.
const GATES: [&str; 2] = [
    "Verify no upstream job failed",
    "Verify no fast gate failed",
];

fn workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/ci.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

/// The `run: |` block of the step named `step`, de-indented.
fn script(step: &str) -> String {
    let text = workflow();
    let lines: Vec<&str> = text.lines().collect();
    let header = format!("- name: {step}");
    let start = lines
        .iter()
        .position(|l| l.trim() == header)
        .unwrap_or_else(|| panic!("no step named {step:?} in ci.yml"));
    let run = (start + 1..lines.len())
        .find(|&i| lines[i].trim() == "run: |")
        .unwrap_or_else(|| panic!("step {step:?} has no `run: |` block"));
    let indent = lines[run].len() - lines[run].trim_start().len() + 2;
    let body: Vec<&str> = lines[run + 1..]
        .iter()
        .take_while(|l| l.trim().is_empty() || l.len() - l.trim_start().len() >= indent)
        .map(|l| l.get(indent..).unwrap_or(""))
        .collect();
    let body = body.join("\n");
    assert!(
        body.contains(RESULTS_EXPR),
        "step {step:?} no longer reads {RESULTS_EXPR}; update this test"
    );
    body
}

/// Runs the gate with `results` and says whether it passed.
fn passes(step: &str, results: &[&str]) -> bool {
    let body = script(step).replace(RESULTS_EXPR, &results.join(" "));
    let out = Command::new("bash")
        .args(["-e", "-c", &body])
        .output()
        .expect("run bash");
    out.status.success()
}

#[test]
fn success_and_skipped_pass() {
    for step in GATES {
        assert!(passes(step, &["success", "skipped", "success"]), "{step}");
        assert!(passes(step, &["skipped"]), "{step}");
    }
}

#[test]
fn every_other_result_fails() {
    // `abandoned` is what the 19 and 20 November nightlies' `CI green` saw for
    // the macOS leg. `timed_out` and `neutral` stand for any result GitHub
    // adds later: the gate must not need to know it.
    for step in GATES {
        for bad in ["failure", "cancelled", "abandoned", "timed_out", "neutral"] {
            let results = ["success", bad, "skipped"];
            assert!(!passes(step, &results), "{step} passed with {results:?}");
        }
    }
}
