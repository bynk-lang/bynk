//! #4125: `bynk new <name> && cd <name> && bynk test` passes. The scaffold's own
//! test must run green as generated, so the starter and its test cannot drift.
//! Runs the real binary; `bynk test` needs `node` and `tsc`, so this skips loudly
//! without them and `BYNK_REQUIRE_TSC=1` turns the skip into a failure, as the
//! other Node-driven suites do.

use std::path::PathBuf;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn required() -> bool {
    std::env::var(REQUIRE_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

fn have(tool: &str) -> bool {
    // npm installs `tsc` as `tsc.cmd` on Windows, which `Command` will not run
    // directly (see bynk-tests' tsc_verify `base_command`), so route it through
    // `cmd /C` there.
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(tool);
        c
    } else {
        Command::new(tool)
    };
    cmd.arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_fresh_project_passes_its_own_test() {
    for tool in ["node", "tsc"] {
        if !have(tool) {
            assert!(
                !required(),
                "NEW STARTER TEST: `{tool}` is missing, and {REQUIRE_ENV} is set"
            );
            eprintln!("NEW STARTER TEST skipped: `{tool}` is missing");
            return;
        }
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("new-starter-test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let bynk = env!("CARGO_BIN_EXE_bynk");

    let new = Command::new(bynk)
        .args(["new", "tproj"])
        .current_dir(&root)
        .output()
        .expect("runs bynk new");
    assert!(
        new.status.success(),
        "{}",
        String::from_utf8_lossy(&new.stderr)
    );
    assert!(root.join("tproj/tests/tproj.test.bynk").is_file());

    let test = Command::new(bynk)
        .arg("test")
        .current_dir(root.join("tproj"))
        .output()
        .expect("runs bynk test");
    let out =
        String::from_utf8_lossy(&test.stdout).into_owned() + &String::from_utf8_lossy(&test.stderr);
    assert!(test.status.success(), "{out}");
    assert!(out.contains("1 passed, 0 failed"), "{out}");
    assert!(!out.contains("no tests found"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}
