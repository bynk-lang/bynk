//! `--color` / `--colour` end to end: `bynk test`'s human output, piped (so
//! `auto` would leave it plain), is coloured under `always` and plain under
//! `never`, coverage table included.
//!
//! The project is a copy of `examples/todo` with one assertion broken, so the
//! run has a pass and a failure. It needs `tsc` and `node`; without them the
//! tests are skipped.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GREEN_TICK: &str = "\x1b[32m\u{2713}\x1b[39m";
const RED: &str = "\x1b[31m";

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_name() == ".bynk" || entry.file_name() == "node_modules" {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

struct Project(PathBuf);

impl Project {
    /// `examples/todo` with its first `expect` made to fail.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bynk-colour-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/todo"),
            &dir,
        );
        let suite = dir.join("tests/todos.bynk");
        let text = std::fs::read_to_string(&suite).unwrap();
        assert!(
            text.contains("\"Buy milk\""),
            "the fixture changed:\n{text}"
        );
        let broken = text.replacen(
            "expect item.title == \"Buy milk\"",
            "expect item.title == \"Buy bread\"",
            1,
        );
        assert_ne!(broken, text, "the fixture changed:\n{text}");
        std::fs::write(&suite, broken).unwrap();
        Project(dir)
    }

    fn test(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bynk"))
            .args(args)
            .arg("test")
            .arg(&self.0)
            .arg("--coverage")
            .env_remove("NO_COLOR")
            .env_remove("FORCE_COLOR")
            .env_remove("CLICOLOR_FORCE")
            .output()
            .unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn runnable() -> bool {
    let ok = have("tsc") && have("node");
    if !ok {
        eprintln!("skipping: `tsc` and `node` are not available");
    }
    ok
}

#[test]
fn always_colours_a_pipe() {
    if !runnable() {
        return;
    }
    let project = Project::new("always");
    for flag in ["--color=always", "--colour=always"] {
        let out = project.test(&[flag]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success(), "a case fails:\n{stdout}");
        assert!(stdout.contains(GREEN_TICK), "{flag}: a green ✓:\n{stdout}");
        assert!(
            stdout.contains(&format!("{RED}  \u{2717} ")),
            "{flag}: a red ✗:\n{stdout}"
        );
        assert!(
            stdout.contains(&format!("{RED}1 failed\x1b[39m")),
            "{flag}: a red tally:\n{stdout}"
        );
        assert!(
            stdout.contains("\x1b[1mCoverage\x1b[22m"),
            "{flag}: a coloured coverage table:\n{stdout}"
        );
    }
}

#[test]
fn never_and_auto_leave_a_pipe_plain() {
    if !runnable() {
        return;
    }
    let project = Project::new("never");
    for args in [&["--color", "never"][..], &[]] {
        let out = project.test(args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("  \u{2713} "), "{args:?}:\n{stdout}");
        assert!(stdout.contains("Coverage"), "{args:?}:\n{stdout}");
        assert!(!stdout.contains('\x1b'), "{args:?}: no ANSI:\n{stdout}");
    }
}
