//! #3814: the driver's own failure messages carry an `error:` label after the
//! speaker (`bynk test: error: …`), so `--color always` paints it red, and a
//! run without the flag, piped, prints no escapes at all.

use std::process::{Command, Output};

const RED_ERROR: &str = "\x1b[31merror:\x1b[39m";

fn bynk(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(args)
        .env_remove("NO_COLOR")
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap()
}

fn missing_dir() -> String {
    std::env::temp_dir()
        .join(format!("bynk-no-such-dir-{}", std::process::id()))
        .display()
        .to_string()
}

#[test]
fn a_failure_is_labelled_error_and_painted_red_under_always() {
    let out = bynk(&["--color", "always", "test", &missing_dir()]);
    assert!(!out.status.success(), "a missing input must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("bynk test: {RED_ERROR} input `")),
        "expected a red `error:` after the speaker, got:\n{stderr:?}"
    );
}

#[test]
fn a_failure_is_labelled_but_plain_without_the_flag() {
    let out = bynk(&["test", &missing_dir()]);
    assert!(!out.status.success(), "a missing input must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bynk test: error: input `"),
        "expected `bynk test: error:`, got:\n{stderr}"
    );
    assert!(
        !stderr.contains('\x1b'),
        "piped stderr must carry no escapes, got:\n{stderr:?}"
    );
}
