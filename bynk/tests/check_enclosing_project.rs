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

    // The note is for the human format: `short` carries none.
    let (ok, text) = check(&dir, &["src", "--format", "short"]);
    assert!(ok, "{text}");
    assert!(!text.contains("checking the project"), "{text}");
}

/// A machine format's stdout stays one clean document under the redirect, and says nothing of it.
#[test]
fn a_machine_format_stays_a_clean_document_under_the_redirect() {
    let dir = scratch("check-enclosing-json", true);
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["check", "src", "--format", "json"])
        .current_dir(&dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bynk");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim_start().starts_with('{'),
        "one JSON document: {stdout}"
    );
    assert!(!stdout.contains("checking the project"), "{stdout}");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("checking the project"));
}

/// Run from above the project, the directory is still resolved to it.
#[test]
fn a_directory_named_from_above_the_project_is_resolved_to_it() {
    let dir = scratch("check-enclosing-above", true);
    let parent = dir.parent().unwrap();
    let name = dir.file_name().unwrap().to_str().unwrap();

    let (ok, text) = check(parent, &[&format!("{name}/src")]);
    assert!(
        ok,
        "`bynk check {name}/src` from the parent reads the manifest:\n{text}"
    );
    assert!(text.contains("checking the project"), "{text}");
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

/// A directory the project does not compile is not the project: `scratch/` is outside `src`, so a
/// check of it must read `scratch`, not return the project's verdict.
#[test]
fn a_directory_outside_the_projects_sources_is_checked_as_itself() {
    let dir = scratch("check-enclosing-outside", true);
    std::fs::create_dir_all(dir.join("scratch")).unwrap();
    // A broken file the project never reads: only a check of `scratch` itself can see it.
    std::fs::write(
        dir.join("scratch/bad.bynk"),
        "context bad\n\nnot a declaration\n",
    )
    .unwrap();

    let (ok, text) = check(&dir, &["scratch"]);
    assert!(!ok, "`scratch` is checked, not the project:\n{text}");
    assert!(!text.contains("checking the project"), "{text}");
    let (ok, text) = check(&dir, &[]);
    assert!(ok, "the project itself is unaffected:\n{text}");
}

/// A directory with a `src/` of its own selects project mode, as the CLI reference says, even
/// inside another project.
#[test]
fn a_directory_with_its_own_src_is_not_replaced_by_the_outer_project() {
    let dir = scratch("check-enclosing-own-src", true);
    let inner = dir.join("src/inner");
    std::fs::create_dir_all(inner.join("src")).unwrap();
    std::fs::write(
        inner.join("src/bad.bynk"),
        "context bad\n\nnot a declaration\n",
    )
    .unwrap();

    let (ok, text) = check(&dir, &["src/inner"]);
    assert!(!ok, "the inner tree is checked:\n{text}");
    assert!(!text.contains("checking the project"), "{text}");
}

/// An `exclude`d subtree is not read by the project, so it is checked as itself.
#[test]
fn an_excluded_directory_is_checked_as_itself() {
    let dir = scratch("check-enclosing-excluded", true);
    let manifest = std::fs::read_to_string(dir.join("bynk.toml")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        format!("{manifest}\n[paths]\ninclude = [\"src\"]\nexclude = [\"src/vendored\"]\n"),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src/vendored")).unwrap();
    std::fs::write(
        dir.join("src/vendored/bad.bynk"),
        "context bad\n\nnot a declaration\n",
    )
    .unwrap();

    let (ok, text) = check(&dir, &["src/vendored"]);
    assert!(!ok, "the excluded tree is checked as itself:\n{text}");
    assert!(!text.contains("checking the project"), "{text}");
}

/// `--fix` edits what it was given: reading the project's manifest widens the check, not the
/// files rewritten.
#[test]
fn fix_stays_inside_the_directory_named() {
    let dir = scratch("check-enclosing-fix", true);
    let outside = dir.join("src/outer.bynk");
    let before = "context outer\n";
    std::fs::write(&outside, before).unwrap();
    std::fs::create_dir_all(dir.join("src/feature")).unwrap();
    std::fs::write(dir.join("src/feature/f.bynk"), "context feature\n").unwrap();

    let _ = check(&dir, &["src/feature", "--fix"]);
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        before,
        "a file outside is untouched"
    );
}
