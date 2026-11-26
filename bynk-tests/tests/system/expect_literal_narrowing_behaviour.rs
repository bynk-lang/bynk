//! #3775: a second `expect x == <literal>` after `expect x == <other literal>` is a runtime
//! failure of that case, not a `tsc` error (TS2367): the first `expect` narrowed `x` to its
//! literal, so a later comparison widens the read. Drives the real `bynk test` CLI against
//! `fixtures/behaviour/expect_literal_narrowing`.
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
fn successive_literal_comparisons_never_rely_on_narrowing() {
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
        .join("tests/fixtures/behaviour/expect_literal_narrowing");
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("expect-literal-narrowing")
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
    assert!(all.contains("✗ eq then other eq fails"), "{all}");
    assert!(all.contains("actual:   1500 == 7"), "{all}");
    for passing in [
        "✓ eq then ne passes",
        "✓ three successive literals",
        "✓ strings and bools",
        "✓ literal on the left",
    ] {
        assert!(all.contains(passing), "{all}");
    }
    for failing in [
        "✗ negative literals",
        "✗ boolean ne then eq",
        "✗ string fails at run time",
    ] {
        assert!(all.contains(failing), "{all}");
    }
    assert!(all.contains("actual:   \"pos\" == \"neg\""), "{all}");
    assert!(all.contains("4 passed, 4 failed."), "{all}");
    assert!(!out.status.success(), "{all}");
}
