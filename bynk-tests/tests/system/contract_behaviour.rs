//! v0.115 behavioural test for the function-contract runner attack (testing
//! track slice 3). Snapshots prove the emitted shape; this proves the runtime
//! behaviour: a contracted function whose `ensures` fails over its `requires`-
//! filtered domain is caught by the runner with a case count, the run's root
//! seed, and a shrunk counterexample carrying a copy-paste reproduce line — and
//! `--seed <hex>` reproduces the run byte-for-byte. A contract is a property
//! that is always on; no `property` is written for it.
//!
//! Drives the real `bynk test` CLI against a fixture project, so it exercises
//! the dev/test guard, the generator/runner, and shrinking end to end. Like the
//! other toolchain-driving tests it skips loudly when no TypeScript runner
//! (`tsc`+`node` or `tsx`) is available; `BYNK_REQUIRE_TSC=1` turns the skip into
//! a failure (CI).

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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/contract_fail")
}

fn run_with_seed(seed: &str, out_dir: &str) -> String {
    // Each run gets its own parent dir: the runner derives the executed
    // `out-js` tree as a *sibling* of `--output`, so a shared parent (e.g. two
    // behavioural tests both rooted directly under `CARGO_TARGET_TMPDIR`)
    // would race the emitted `.js` a concurrent test's `tsc`/`node` run reads.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .arg("--seed")
        .arg(seed)
        .output()
        .expect("run bynk test");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[test]
fn failing_contract_is_caught_by_the_runner_with_seed_and_shrunk_counterexample() {
    // `bynk test`'s runner detection is Unix-only (it shells out to `which`), so
    // the CLI cannot locate a runner on Windows and never runs the emitted tests.
    // This test drives that CLI end to end — skip on Windows. (The *emission* is
    // covered platform-independently by the golden fixture `244_contract_passes`.)
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

    let first = run_with_seed("0x5f3a", "contract-out-a");

    // The runner attack names the contract and reports the failure like a
    // property: a case count, the passed root seed, a shrunk counterexample, and
    // a copy-paste reproduce line.
    assert!(
        first.contains("contract dec"),
        "expected the contract-attack runner to be named `contract dec`, got:\n{first}"
    );
    assert!(
        first.contains("property failed after"),
        "expected a failure banner, got:\n{first}"
    );
    assert!(
        first.contains("(seed 0x5f3a)"),
        "expected the passed root seed in the report, got:\n{first}"
    );
    assert!(
        first.contains("shrunk counterexample:"),
        "expected a shrunk counterexample, got:\n{first}"
    );
    assert!(
        first.contains("contract violated: postcondition `stays_nonneg`"),
        "expected the violated postcondition in the detail, got:\n{first}"
    );
    assert!(
        first.contains("--seed 0x5f3a"),
        "expected a reproduce line carrying the root seed, got:\n{first}"
    );

    // The minimal counterexample: `x == 0` (the only `requires`-satisfying input
    // for which `x - 1 < 0`). Deterministic under the seed.
    assert!(
        first.contains("x = 0"),
        "expected the shrunk counterexample `x = 0`, got:\n{first}"
    );

    // Re-running with the same seed reproduces the same counterexample.
    let second = run_with_seed("0x5f3a", "contract-out-b");
    let extract = |s: &str| {
        s.lines()
            .find(|l| l.contains("shrunk counterexample:"))
            .map(|l| l.trim().to_string())
    };
    assert_eq!(
        extract(&first),
        extract(&second),
        "the same seed must reproduce the same shrunk counterexample"
    );
}
