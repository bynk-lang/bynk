//! #3013 (track E3): `bynk` answers to the name `bynkc`, end to end.
//!
//! The release archives ship a copy of `bynk` named `bynkc`, and the dispatch is on
//! the executable's file name, so the path that ships is a real copy run by name:
//! not a function call. These tests make that copy and run it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MANIFEST: &str = "[project]\nname = \"calc\"\nversion = \"0.1.0\"\n";
const SOURCE: &str = "commons calc {\n  fn dbl(n: Int) -> Int { n + n }\n}\n";
const NOTICE: &str = "bynkc: notice:";

/// A scratch directory holding a copy of the built `bynk` under `name`, and a
/// minimal project beside it.
fn setup(test: &str, name: &str) -> (PathBuf, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let exe = dir
        .join("bin")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_bynk"), &exe).unwrap();
    let project = dir.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("bynk.toml"), MANIFEST).unwrap();
    std::fs::write(project.join("src/calc.bynk"), SOURCE).unwrap();
    (exe, project)
}

fn run(exe: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .env_remove("BYNK_BYNKC")
        .output()
        .expect("run the copy")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_copy_named_bynkc_runs_bynkcs_commands_and_prints_the_notice_once() {
    let (exe, project) = setup("alias-check", "bynkc");
    let out = run(&exe, &project, &["check", "."]);
    assert!(out.status.success(), "stderr: {}", text(&out.stderr));
    assert_eq!(
        text(&out.stderr).matches(NOTICE).count(),
        1,
        "one notice: {}",
        text(&out.stderr)
    );
    assert!(
        text(&out.stderr).contains("retired"),
        "{}",
        text(&out.stderr)
    );

    // `compile` takes `bynkc`'s flags and writes the output tree.
    let out = run(
        &exe,
        &project,
        &["compile", ".", "-o", "out", "--emit", "js"],
    );
    assert!(out.status.success(), "stderr: {}", text(&out.stderr));
    assert!(
        project.join("out/calc.js").exists(),
        "the JavaScript is written"
    );
}

#[test]
fn a_copy_named_bynkc_reports_itself_as_bynkc_without_the_notice() {
    let (exe, project) = setup("alias-version", "bynkc");
    for args in [&["--version"][..], &["--help"][..]] {
        let out = run(&exe, &project, args);
        assert!(out.status.success(), "{args:?}");
        assert!(
            text(&out.stdout).contains("bynkc"),
            "{args:?}: {}",
            text(&out.stdout)
        );
        assert!(
            !text(&out.stderr).contains(NOTICE),
            "{args:?} stays byte-for-byte: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn bynk_under_its_own_name_is_unchanged() {
    let (exe, project) = setup("alias-not", "bynk");
    let out = run(&exe, &project, &["check", "."]);
    assert!(out.status.success(), "stderr: {}", text(&out.stderr));
    assert!(!text(&out.stderr).contains(NOTICE), "{}", text(&out.stderr));
    let version = run(&exe, &project, &["--version"]);
    assert!(
        text(&version.stdout).starts_with("bynk "),
        "{}",
        text(&version.stdout)
    );
}
