//! v0.127 behavioural test for `bynk test --case <name>` (editor-currency slice
//! 6). The per-case run filter behind the editor's `▷ Run Test` lens: with
//! `--case` set, only cases whose name matches execute — not merely reported,
//! executed. This drives the real `bynk test` CLI against a two-case fixture and
//! asserts the sibling case does not run.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner (`tsc`+`node` or `tsx`) is available; `BYNK_REQUIRE_TSC=1` turns the
//! skip into a failure (CI).

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
/// PATH — the same fallback chain `bynk test` itself walks.
fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/case_filter")
}

/// Run `bynk test <fixture>` with optional `--case <name>` and return combined
/// stdout+stderr.
fn run(case: Option<&str>, out_dir: &str) -> String {
    // Each run gets its own parent dir: the runner derives the executed
    // `out-js` tree as a *sibling* of `--output`, so a shared parent (e.g. two
    // behavioural tests both rooted directly under `CARGO_TARGET_TMPDIR`)
    // would race the emitted `.js` a concurrent test's `tsc`/`node` run reads.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"));
    cmd.arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root);
    if let Some(name) = case {
        cmd.arg("--case").arg(name);
    }
    let out = cmd.output().expect("run bynk test");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[test]
fn case_filter_runs_only_the_named_case() {
    // `bynk test`'s runner detection is Unix-only (it shells out to `which`), so
    // this end-to-end test is meaningful only where the CLI can locate a runner.
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    // Unfiltered, both cases run.
    let all = run(None, "out-all");
    assert!(
        all.contains("doubles two") && all.contains("doubles three"),
        "an unfiltered run executes both cases, got:\n{all}"
    );
    assert!(
        all.contains("2 passed, 0 failed."),
        "both cases pass unfiltered, got:\n{all}"
    );

    // Filtered to one case, only that case executes — the sibling is absent and
    // exactly one case passes.
    let one = run(Some("doubles three"), "out-one");
    assert!(
        one.contains("doubles three"),
        "the named case runs, got:\n{one}"
    );
    assert!(
        !one.contains("doubles two"),
        "the sibling case must not run under the filter, got:\n{one}"
    );
    assert!(
        one.contains("1 passed, 0 failed."),
        "exactly one case runs under the filter, got:\n{one}"
    );
}
