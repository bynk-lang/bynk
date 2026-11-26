//! #3826: `bynk check <dir>` inside a project reads the project's `bynk.toml`.
//!
//! A directory with no manifest of its own, below one that has it, is checked as that project, so
//! `bynk check src` sees the `[unstable] allow` the manifest carries instead of checking `src` as a
//! bare tree and sending the reader to edit a file the run never read. Driven through the binary:
//! the working directory and the walk up are the behaviour.

use std::path::{Path, PathBuf};
use std::process::Command;

const API: &str = "\
context api

consumes bynk.crypto { Crypto }

service api from http {
  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok(\"pong\")
  }
}
";

/// A project whose manifest allows `bynk.crypto`, which its one source consumes.
fn scratch(name: &str, allow: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let unstable = if allow {
        "\n[unstable]\nallow = [\"bynk.crypto\"]\n"
    } else {
        ""
    };
    std::fs::write(
        dir.join("bynk.toml"),
        format!("[project]\nname = \"enclosing\"\nversion = \"0.1.0\"\n{unstable}"),
    )
    .unwrap();
    std::fs::write(dir.join("src/api.bynk"), API).unwrap();
    dir
}

/// `bynk check <args>` in `cwd`: (success, stdout + stderr).
fn check(cwd: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .arg("check")
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bynk");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn a_subdirectory_of_a_project_is_checked_with_the_projects_manifest() {
    let dir = scratch("check-enclosing-allowed", true);

    let (ok, text) = check(&dir, &[]);
    assert!(ok, "the project checks:\n{text}");

    // The issue's case: `src` from the project root.
    let (ok, text) = check(&dir, &["src"]);
    assert!(ok, "`bynk check src` reads the manifest:\n{text}");
    assert!(text.contains("checking the project"), "it says so: {text}");

    // From inside the tree, with the directory named relative to it.
    let (ok, text) = check(&dir.join("src"), &["."]);
    assert!(ok, "`bynk check .` in `src` reads the manifest:\n{text}");

    // A machine format keeps stdout to the document.
    let (ok, text) = check(&dir, &["src", "--format", "short"]);
    assert!(ok, "{text}");
    assert!(!text.contains("checking the project"), "{text}");
}

#[test]
fn the_manifest_still_refuses_what_it_does_not_allow() {
    let dir = scratch("check-enclosing-refused", false);

    let (ok, text) = check(&dir, &["src"]);
    assert!(!ok, "the project refuses `bynk.crypto`:\n{text}");
    assert!(text.contains("bynk.stability.experimental"), "{text}");
    // The same answer as the plain project check, so the note is true of the run.
    let (plain_ok, plain) = check(&dir, &[]);
    assert!(!plain_ok, "{plain}");
    assert!(plain.contains("bynk.stability.experimental"), "{plain}");
}
