//! #3639: a suite of 500 or more cases type-checks and runs.
//!
//! The generated runner (`export async function run(only?, onCase?)`) has one
//! `if` block per case. Its accumulator was declared `const results = [];`, an
//! evolving array whose type `tsc` infers by control-flow analysis over the
//! whole function, and at 500 blocks that passes TypeScript's flow-node limit:
//! `TS2563: The containing function or module body is too large for control flow
//! analysis`, reported as `bynk.codegen.tsc_failed`. Typing the accumulator as
//! `CaseResult[]` needs no such analysis.
//!
//! The project is built here, not checked in: one commons function and a suite
//! of `CASES` trivial cases, run through the real `bynk test`. Like the other
//! toolchain-driving tests it skips loudly when no TypeScript runner is
//! available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).

#[path = "../require/mod.rs"]
mod require;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

/// Past the limit (500) with a margin, so the test does not sit on the boundary.
const CASES: usize = 520;

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The fallback chain `bynk test` itself walks.
fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

/// Write the project: `demo.m` with one function, and a suite of `cases` cases.
fn write_project(cases: usize) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("large-suite");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/demo")).unwrap();
    std::fs::create_dir_all(root.join("src/tests")).unwrap();
    std::fs::write(root.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(
        root.join("src/demo/m.bynk"),
        "commons demo.m\n\nfn double(n: Int) -> Int {\n  n * 2\n}\n",
    )
    .unwrap();
    let mut suite = String::from("suite demo.m {\n");
    for i in 0..cases {
        let _ = write!(
            suite,
            "  case \"doubles {i}\" {{\n    expect double({i}) == {}\n  }}\n\n",
            i * 2
        );
    }
    suite.push_str("}\n");
    std::fs::write(root.join("src/tests/m.test.bynk"), suite).unwrap();
    root
}

#[test]
fn a_suite_of_more_than_500_cases_type_checks_and_runs() {
    // `bynk test`'s runner detection is Unix-only (it shells out to `which`).
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if require::is_required(REQUIRE_ENV) {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    let root = write_project(CASES);
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(&root)
        .arg("--output")
        .arg(root.join("build").join("out"))
        .output()
        .expect("run bynk test");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));

    assert!(
        !text.contains("tsc_failed") && !text.contains("TS2563"),
        "a {CASES}-case suite must type-check, got:\n{text}"
    );
    assert!(
        text.contains(&format!("{CASES} passed, 0 failed.")),
        "every case runs and passes, got:\n{text}"
    );
    assert!(out.status.success(), "bynk test succeeds, got:\n{text}");
}
