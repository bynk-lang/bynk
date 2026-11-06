//! #46: `bynk check` / `bynk compile` root a directory project the same way
//! `bynk test` does — a `bynk.toml` (or a `src/` subdir) selects split-paths
//! mode, so running them from the **project root** of a conventional layout
//! works, not just from `src/`. Driven through the real `bynk` binary.

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli_rooting")
}

fn bynk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
}

/// `bynk check <project-root>` honours `bynk.toml`/`src` and succeeds — the
/// regression: before #46 it treated `<root>` as a single tree and errored
/// (`inconsistent_commons_name`, naming `src/`-prefixed paths).
#[test]
fn check_honors_split_layout_from_the_project_root() {
    let out = bynk()
        .arg("check")
        .arg(fixture())
        .output()
        .expect("run bynk check");
    assert!(
        out.status.success(),
        "expected a clean check from the project root; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Pointing at the `src/` subdir directly still works (single-tree on `src/`) —
/// the pre-#46 behaviour is unchanged for that invocation.
#[test]
fn check_still_works_when_pointed_at_src() {
    let out = bynk()
        .arg("check")
        .arg(fixture().join("src"))
        .output()
        .expect("run bynk check");
    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
