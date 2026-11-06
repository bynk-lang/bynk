//! Locale capability track, slice 1 (#844) review follow-up: `e2e.rs`'s
//! `positive_fixtures`/`bless_positive_fixtures` are golden-text diffs only,
//! and `tsc_verify` type-checks against `app_greet as any` — neither actually
//! *runs* a `stub` override on a capability flattened in from an adapter
//! (`consumes bynk { Locale }` + bare `given Locale`). That combination had a
//! real bug (`emit_test_deps` never wired the flattened capability into
//! `makeTestDeps()`, so the stub class was generated but never instantiated —
//! `deps.Locale` was `undefined` and the test threw at runtime instead of
//! exercising the stub) that no check in the suite could catch. This drives
//! the real `bynkc test` CLI against fixture 807 so a regression of that class
//! fails CI, not just a manual re-run.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

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

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/positive/{name}/src"))
}

fn run(fixture_src: &PathBuf, out_dir: &str) -> String {
    // Each run gets its own parent dir: the runner derives the executed
    // `out-js` tree as a *sibling* of `--output`, so a shared parent (e.g. two
    // behavioural tests both rooted directly under `CARGO_TARGET_TMPDIR`)
    // would race the emitted `.js` a concurrent test's `tsc`/`node` run reads.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(fixture_src)
        .arg("--output")
        .arg(&out_root)
        .output()
        .expect("run bynkc test");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

// One `#[test]` running both fixtures sequentially, matching the sibling
// toolchain-driving files (`case_filter.rs`, `contract_behaviour.rs`, …),
// each of which has exactly one `#[test]` — concurrent `bynkc test`
// invocations from the same binary race each other (observed: interleaved
// stdout from two runs, unrelated to this fix), so this repo's convention is
// one CLI-driving `#[test]` per file rather than chasing that down here.
#[test]
fn adapter_flattened_capability_stubs_actually_run() {
    // `bynkc test`'s runner detection is Unix-only, like the sibling
    // toolchain-driving tests.
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

    let stub_out = run(&fixture("807_locale_stub_override"), "out-807");
    assert!(
        stub_out.contains("a stubbed locale is what the capability reports")
            && stub_out.contains("render is unaffected by the stub in slice 1 (tag is unused)"),
        "both cases should have run, got:\n{stub_out}"
    );
    assert!(
        stub_out.contains("2 passed, 0 failed."),
        "the stub must actually take effect at runtime (a dead stub throws \
         `Cannot read properties of undefined`), got:\n{stub_out}"
    );

    let basic_out = run(&fixture("804_locale_render_basic"), "out-804");
    assert!(
        basic_out.contains("3 passed, 0 failed."),
        "all three render-determinism cases should pass at runtime, got:\n{basic_out}"
    );
}
