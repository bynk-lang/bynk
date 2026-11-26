//! #4124: `bynk fmt` with no paths formats the enclosing project, the same file
//! set `bynk check` reads (`[paths] include` minus `exclude`), for `fmt` and
//! `fmt --check`. Outside a project it keeps its old error, and `-` is still
//! stdin. Runs the real binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const UGLY: &str = "context   demo {\n}\n";
const CANONICAL: &str = "context demo {\n}\n";
const NO_INPUT: &str =
    "bynk fmt: error: no input files (pass file or directory paths, or `-` for stdin)";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// `bynk <args>` in `cwd`, optionally feeding `stdin`: (exit code, stdout, stderr).
fn bynk(cwd: &Path, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk"));
    cmd.args(args)
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawns bynk");
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A project whose `[paths]` includes `src` and `lib` and excludes `lib/vendor`,
/// with an unformatted file in each tree plus one outside `include`.
fn project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("bynk.toml"),
        "[project]\nname = \"p\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\", \"lib\"]\nexclude = [\"lib/vendor\"]\n",
    );
    write(&root.join("src/a.bynk"), UGLY);
    write(&root.join("lib/b.bynk"), UGLY);
    write(&root.join("lib/vendor/c.bynk"), UGLY);
    write(&root.join("other/d.bynk"), UGLY);
    root
}

#[test]
fn no_paths_formats_the_included_trees_and_leaves_the_rest() {
    let root = project("fmt-default-write");
    let (code, _, err) = bynk(&root, &["fmt"], None);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read(&root.join("src/a.bynk")), CANONICAL);
    assert_eq!(read(&root.join("lib/b.bynk")), CANONICAL);
    // `exclude` and files outside `include` are `check`'s blind spots too.
    assert_eq!(read(&root.join("lib/vendor/c.bynk")), UGLY);
    assert_eq!(read(&root.join("other/d.bynk")), UGLY);
}

#[test]
fn no_paths_check_reports_without_writing_then_passes_once_formatted() {
    let root = project("fmt-default-check");
    let (code, _, err) = bynk(&root, &["fmt", "--check"], None);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("src/a.bynk"), "{err}");
    assert!(err.contains("lib/b.bynk"), "{err}");
    assert!(!err.contains("vendor"), "{err}");
    assert!(!err.contains("d.bynk"), "{err}");
    assert_eq!(
        read(&root.join("src/a.bynk")),
        UGLY,
        "--check must not write"
    );

    assert_eq!(bynk(&root, &["fmt"], None).0, 0);
    let (code, _, err) = bynk(&root, &["fmt", "--check"], None);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn no_paths_finds_the_project_from_a_subdirectory() {
    let root = project("fmt-default-subdir");
    let (code, _, err) = bynk(&root.join("src"), &["fmt"], None);
    assert_eq!(code, 0, "{err}");
    // The whole project, not just the directory the command ran in.
    assert_eq!(read(&root.join("src/a.bynk")), CANONICAL);
    assert_eq!(read(&root.join("lib/b.bynk")), CANONICAL);
}

#[test]
fn outside_a_project_the_no_input_error_stands() {
    // `temp_dir`, not the crate tmpdir: that sits under the repo, and the test
    // wants a directory with no `bynk.toml` above it.
    let dir = std::env::temp_dir().join(format!("bynk-fmt-no-project-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for args in [&["fmt"][..], &["fmt", "--check"][..]] {
        let (code, _, err) = bynk(&dir, args, None);
        assert_eq!(code, 1, "{err}");
        assert!(err.contains(NO_INPUT), "{err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dash_is_still_stdin_inside_a_project() {
    let root = project("fmt-default-stdin");
    let (code, out, err) = bynk(&root, &["fmt", "-"], Some(UGLY));
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, CANONICAL);
    // Stdin mode touches no project file.
    assert_eq!(read(&root.join("src/a.bynk")), UGLY);
}

#[test]
fn explicit_paths_still_win_over_the_project_default() {
    let root = project("fmt-default-explicit");
    let (code, _, err) = bynk(&root, &["fmt", "other/d.bynk"], None);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read(&root.join("other/d.bynk")), CANONICAL);
    assert_eq!(read(&root.join("src/a.bynk")), UGLY);
}

/// A workspace of members `a` and `b` (each a package with one unformatted
/// file), the shape `bynk check` and `fmt <root>` already expand.
fn workspace(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\n",
    );
    for member in ["a", "b"] {
        write(
            &root.join(member).join("bynk.toml"),
            &format!("[package]\nname = \"acme/{member}\"\n"),
        );
        write(&root.join(member).join(format!("src/{member}.bynk")), UGLY);
    }
    root
}

#[test]
fn from_a_workspace_root_every_member_is_formatted() {
    let root = workspace("fmt-default-workspace-root");
    let (code, _, err) = bynk(&root, &["fmt"], None);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read(&root.join("a/src/a.bynk")), CANONICAL);
    assert_eq!(read(&root.join("b/src/b.bynk")), CANONICAL);
}

#[test]
fn from_inside_a_member_only_that_member_is_formatted() {
    // The walk stops at the nearest `bynk.toml`, which is the member's own: a
    // member is a project, and `fmt` run in it means that project.
    let root = workspace("fmt-default-workspace-member");
    let (code, _, err) = bynk(&root.join("a/src"), &["fmt"], None);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read(&root.join("a/src/a.bynk")), CANONICAL);
    assert_eq!(read(&root.join("b/src/b.bynk")), UGLY);
}

#[test]
fn piped_input_without_dash_is_ignored_and_the_project_is_formatted() {
    // Unlike gofmt and rustfmt, stdin is read only for `-`. Piped text with no
    // paths used to be the no-input error; inside a project it now formats the
    // project, and the pipe is left alone.
    let root = project("fmt-default-pipe-no-dash");
    let (code, out, err) = bynk(&root, &["fmt"], Some(UGLY));
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "", "nothing is written to stdout");
    assert_eq!(read(&root.join("src/a.bynk")), CANONICAL);
}
