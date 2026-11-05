//! #2857: `?` on a `Result` in a test `case` fails the case (reporting the `Err`
//! and the `?`'s location) instead of lowering to an early `return` of the
//! `Err`, which `tsc` rejected against the case's `CaseResult`. Drives the real
//! `bynkc test` CLI against `fixtures/behaviour/question_in_case`.
//!
//! Like the other toolchain-driving tests it skips on Windows and, loudly, when
//! no TypeScript runner is on `PATH`; `BYNK_REQUIRE_TSC` makes the skip a failure.

use std::path::PathBuf;
use std::process::Command;

mod require;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

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

#[test]
fn question_on_an_err_fails_the_case() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if require::is_required(REQUIRE_ENV) {
            panic!("no TypeScript runner on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/question_in_case");
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("question-in-case")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc-under-test"))
        .arg("test")
        .arg(&fixture)
        .arg("--output")
        .arg(&out_root)
        .output()
        .expect("run bynkc test");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Ran to completion (no `tsc` error), failing exactly the two `Err` cases.
    assert!(
        !all.contains("TS2345") && !all.contains("tsc reported"),
        "{all}"
    );
    assert!(all.contains("✓ ? on an Ok unwraps the value"), "{all}");
    assert!(all.contains("✗ ? on an Err fails the case"), "{all}");
    assert!(all.contains("✗ ? in a match arm fails the case"), "{all}");
    assert!(all.contains("the operand was Err:"), "{all}");
    assert!(all.contains("must be non-empty"), "{all}");
    assert!(all.contains("at tests/day.bynk:9:10"), "{all}");
    assert!(all.contains("1 passed, 2 failed."), "{all}");
    assert!(!out.status.success(), "{all}");
}
