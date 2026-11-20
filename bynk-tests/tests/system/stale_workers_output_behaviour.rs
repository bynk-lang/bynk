//! #3748: `bynk test` after a Workers build into the same output directory.
//!
//! A Workers build (`bynk build --target workers`, or the one `bynk test
//! --against` runs) writes `workers/<worker>/` entry points that import the
//! commons codecs (`__serialise_*`). An in-process `bynk test` then rewrote
//! the commons modules without those codecs, since nothing in-process needs
//! them, but left `workers/` in place. The emitted `tsconfig.json` includes
//! every `.ts` under the output root, so `tsc` checked the old Worker files
//! against the new modules and failed with `bynk.codegen.tsc_failed`. A test
//! run now prunes what it did not write, as `bynk build` does.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

use std::path::{Path, PathBuf};
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
    (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn bynk(args: &[&std::ffi::OsStr]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(args)
        .output()
        .expect("run bynk");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn a_test_run_after_a_workers_build_ignores_the_stale_worker_files() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsc+node) on PATH");
        return;
    }
    // A commons type stored by a context's agent, so the Workers build emits
    // codec imports that the in-process build's commons module does not
    // export; and unit tests only, so `bynk test` builds no Workers output of
    // its own to paper over the stale files.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/1829_agent_state_unread_commons_field");
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("stale-workers-output");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    for part in ["bynk.toml", "src", "tests"] {
        let from = fixture.join(part);
        if from.is_dir() {
            copy_tree(&from, &project.join(part));
        } else {
            std::fs::create_dir_all(&project).unwrap();
            std::fs::copy(&from, project.join(part)).unwrap();
        }
    }
    // Both commands' default output directory, as in the issue.
    let out = project.join("out");

    let (built, log) = bynk(&[
        "build".as_ref(),
        "--target".as_ref(),
        "workers".as_ref(),
        project.as_os_str(),
    ]);
    assert!(built, "the Workers build should succeed:\n{log}");
    assert!(
        out.join("workers").is_dir(),
        "the Workers build should write `workers/`:\n{log}"
    );

    let (passed, log) = bynk(&["test".as_ref(), project.as_os_str()]);
    assert!(
        passed && log.contains("1 passed, 0 failed."),
        "`bynk test` should pass after a Workers build into the same output directory:\n{log}"
    );
    assert!(
        !out.join("workers").exists(),
        "the test run should prune the Workers build's `workers/` tree"
    );
}
