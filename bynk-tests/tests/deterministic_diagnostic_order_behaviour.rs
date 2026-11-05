//! Regression: cross-unit diagnostic order must be deterministic run to run.
//!
//! `groups`/`kinds`/`test_groups`/`integration_groups`/`unit_info` were plain
//! `HashMap`s keyed by unit name, and the per-unit check loop iterates them in
//! insertion order — so which unit's errors landed first in the shared
//! `ErrorSink` depended on `HashMap`'s randomised iteration order, varying
//! between process runs of the identical source. Switching those five maps to
//! `BTreeMap` makes the iteration (and so the diagnostic order) a function of
//! the unit names alone.
//!
//! Drives the real `bynkc check` CLI repeatedly against a fixture with five
//! independent commons units, each producing exactly one `unknown_type` error,
//! and asserts every run's `--format short` output is byte-identical.

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/behaviour/deterministic_diagnostic_order")
}

fn run_once() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc-under-test"))
        .arg("check")
        .arg(fixture())
        .arg("--format")
        .arg("short")
        .output()
        .expect("run bynkc check");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[test]
fn cross_unit_diagnostic_order_is_stable_across_runs() {
    // Each run is a fresh process (a fresh `HashMap` random seed, when it was
    // one) — enough repetitions that pre-fix runs reliably showed more than
    // one distinct order.
    let first = run_once();
    assert!(
        first.contains("unit_a.bynk") && first.contains("unit_e.bynk"),
        "expected all five units' errors, got:\n{first}"
    );
    for n in 1..15 {
        let next = run_once();
        assert_eq!(
            first, next,
            "run {n} produced a different diagnostic order than run 0 — \
             cross-unit order is not deterministic"
        );
    }
    // The five units' errors should also appear in name order — the direct,
    // observable consequence of iterating a `BTreeMap` rather than a
    // `HashMap` keyed by unit name.
    let positions: Vec<usize> = ["unit_a", "unit_b", "unit_c", "unit_d", "unit_e"]
        .iter()
        .map(|u| {
            first
                .find(u)
                .unwrap_or_else(|| panic!("expected `{u}` in output:\n{first}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "expected units in name order in the output, got:\n{first}"
    );
}
