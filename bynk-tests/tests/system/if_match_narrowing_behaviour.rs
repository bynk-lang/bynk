//! #4520: a comparison that re-tests a read an enclosing `if` or `match` arm narrowed to a literal
//! type-checks (it was TS2367, `bynk.codegen.tsc_failed`, on valid Bynk). Drives the real
//! `bynk test` CLI against `fixtures/behaviour/if_match_narrowing`.
//!
//! Skips on Windows and, loudly, when no TypeScript runner is on `PATH`; `BYNK_REQUIRE_TSC`
//! makes the skip a failure.

use std::path::PathBuf;
use std::process::Command;

#[path = "../require/mod.rs"]
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
fn a_retest_of_a_narrowed_read_type_checks_and_runs() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if require::is_required(REQUIRE_ENV) {
            panic!("no TypeScript runner on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/behaviour/if_match_narrowing");
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if-match-narrowing")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(&fixture)
        .arg("--output")
        .arg(&out_root)
        .output()
        .expect("run bynk test");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !all.contains("TS2367") && !all.contains("tsc reported"),
        "{all}"
    );
    assert!(all.contains("✓ all"), "{all}");
    assert!(all.contains("1 passed, 0 failed."), "{all}");
    assert!(out.status.success(), "{all}");
}
