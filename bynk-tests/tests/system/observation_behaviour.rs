//! v0.117 behavioural test for the observation surface (testing track slice 5).
//! Golden fixtures prove the emitted shape; this proves the runtime behaviour:
//! calls are recorded at the capability seam in the test build, the `expect`
//! sugar (`called` / count / `with` / `before`) reads that recording, and the
//! `trace(Cap.op)` escape hatch agrees with the sugar — same count, same
//! arguments, in call order.
//!
//! Drives the real `bynkc test` CLI against a fixture project, so it exercises
//! the recording proxy, the sugar lowering, and the trace lowering end to end.
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner (`tsx` or `tsc`+`node` or `npx`) is available; `BYNK_REQUIRE_TSC=1`
//! turns the skip into a failure (CI).

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

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/observation")
}

#[test]
fn observation_sugar_and_trace_agree_at_runtime() {
    // `bynkc test`'s runner detection is Unix-only (it shells out to `which`), so
    // the CLI cannot locate a runner on Windows and never runs the emitted tests.
    // This test drives that CLI end to end — skip on Windows. (The *emission* is
    // covered platform-independently by the golden fixture `246_observation_surface`.)
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

    // Each run gets its own parent dir: the runner derives the executed
    // `out-js` tree as a *sibling* of `--output`, so a shared parent (e.g. two
    // behavioural tests both rooted directly under `CARGO_TARGET_TMPDIR`)
    // would race the emitted `.js` a concurrent test's `tsc`/`node` run reads.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("observation-out")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .output()
        .expect("run bynkc test");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));

    // The case exercises every sugar form plus a `trace(Cap.op)` cross-check; a
    // pass means the recording proxy, the sugar, and the trace all agree.
    assert!(
        report.contains("sugar and trace agree on the recorded calls"),
        "expected the observation case to be reported, got:\n{report}"
    );
    assert!(
        report.contains("1 passed, 0 failed"),
        "expected the observation case to pass, got:\n{report}"
    );
    assert!(
        out.status.success(),
        "expected `bynkc test` to exit 0, got:\n{report}"
    );
}
