//! #2886: a context capability taking commons types (record, refinement, enum,
//! nested in `List`/`Option`) must emit call-record types `tsc` accepts. Drives
//! the real `bynkc test` CLI against `fixtures/behaviour/commons_type_in_capability`.
//! Skips when no TypeScript runner is on `PATH`; `BYNK_REQUIRE_TSC` makes that a failure.

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
fn commons_types_in_capability_ops_typecheck() {
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
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/behaviour/commons_type_in_capability");
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("commons-type-in-capability")
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
    assert!(
        !all.contains("TS2304") && !all.contains("tsc reported"),
        "{all}"
    );
    assert!(all.contains("3 passed, 0 failed."), "{all}");
    assert!(out.status.success(), "{all}");
}
