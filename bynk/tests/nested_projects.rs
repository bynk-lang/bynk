//! #3242: a nested `bynk.toml` marks a project boundary. `check` and `test` in a
//! directory that is no project but holds projects refuse and say what to do,
//! discovery never walks into another project, and a workspace root, whose
//! members are such projects, is unaffected. `fmt` formats a directory of
//! projects as one tree, each file by its own project's settings.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("nested-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn run(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bynk())
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn bynk");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A project named `name` under `root`: a manifest and one unit.
fn project(root: &Path, name: &str) {
    write(
        &root.join(name).join("bynk.toml"),
        &format!("[project]\nname = \"{name}\"\n"),
    );
    write(
        &root.join(name).join("src").join(format!("{name}.bynk")),
        &format!("commons {name} {{\n\tfn dbl(n: Int) -> Int {{\n\t\tn + n\n\t}}\n}}\n"),
    );
}

/// A directory with no manifest holding `names` as projects.
fn projects_dir(test: &str, names: &[&str]) -> PathBuf {
    let root = scratch(test);
    for name in names {
        project(&root, name);
    }
    root
}

#[test]
fn check_refuses_a_directory_of_projects_and_names_them() {
    let root = projects_dir("check", &["alpha", "beta", "gamma"]);
    let (code, _out, err) = run(&root, &["check", "."]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(
        err.contains("not a project") && err.contains("3 projects"),
        "{err}"
    );
    for name in ["`alpha`", "`beta`", "`gamma`"] {
        assert!(err.contains(name), "names {name}: {err}");
    }
    assert!(
        err.contains("[workspace]") && err.contains("members = [\"*\"]"),
        "{err}"
    );
    assert!(err.contains("inside one"), "{err}");
}

#[test]
fn test_refuses_a_directory_of_projects_too() {
    let root = projects_dir("test", &["alpha", "beta"]);
    let (code, _out, err) = run(&root, &["test", "."]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(
        err.contains("not a project") && err.contains("2 projects"),
        "{err}"
    );
    assert!(err.contains("`alpha`") && err.contains("`beta`"), "{err}");
}

#[test]
fn the_refusal_names_the_first_few_and_counts_the_rest() {
    let names = ["p1", "p2", "p3", "p4", "p5", "p6", "p7"];
    let root = projects_dir("many", &names);
    let (code, _out, err) = run(&root, &["check", "."]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(err.contains("7 projects"), "{err}");
    assert!(err.contains("`p5`") && !err.contains("`p6`"), "{err}");
    assert!(err.contains("and 2 more"), "{err}");
}

/// The refusal also covers a nested layout (`group/leaf`), and a single nested
/// project is named as one.
#[test]
fn a_single_project_is_named_as_a_project() {
    let root = scratch("single");
    project(&root.join("group"), "leaf");
    let (code, _out, err) = run(&root, &["check", "."]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(
        err.contains("holds a project") && err.contains("`group/leaf`"),
        "{err}"
    );
}

/// A directory of plain `.bynk` files, with no nested project, still checks as
/// the one tree it always did.
#[test]
fn a_plain_directory_is_not_refused() {
    let root = scratch("plain");
    write(
        &root.join("calc.bynk"),
        "commons calc {\n  fn dbl(n: Int) -> Int { n + n }\n}\n",
    );
    let (code, _out, err) = run(&root, &["check", "."]);
    assert_eq!(code, 0, "stderr:\n{err}");
}

/// Inside a real project, a subdirectory with its own manifest is another
/// project: its files are not part of this one, and a one-line note says so.
#[test]
fn a_nested_project_is_skipped_with_a_note() {
    let root = scratch("skip");
    write(
        &root.join("bynk.toml"),
        "[project]\nname = \"outer\"\n\n[paths]\ninclude = [\".\"]\n",
    );
    write(
        &root.join("outer.bynk"),
        "commons outer {\n  fn dbl(n: Int) -> Int { n + n }\n}\n",
    );
    // The nested project would not compile as part of `outer`.
    project(&root.join("vendor"), "inner");
    write(
        &root
            .join("vendor")
            .join("inner")
            .join("src")
            .join("inner.bynk"),
        "commons inner\n\nlet x: Int = \"nope\"\n",
    );
    let (code, _out, err) = run(&root, &["check", "."]);
    assert_eq!(
        code, 0,
        "the nested project's error is not ours; stderr:\n{err}"
    );
    assert!(
        err.contains("skipped `vendor/inner`") && err.contains("own bynk.toml"),
        "{err}"
    );
    // And a run inside it still checks it.
    let (code, _out, err) = run(&root.join("vendor").join("inner"), &["check", "."]);
    assert_eq!(code, 1, "the project itself is checked; stderr:\n{err}");
}

/// A directory with no manifest of its own, inside a project, is not refused:
/// the project above it is the context.
#[test]
fn a_manifestless_directory_inside_a_project_is_not_refused() {
    let root = scratch("inside");
    write(&root.join("bynk.toml"), "[project]\nname = \"outer\"\n");
    let sub = root.join("tools");
    project(&sub, "inner");
    let (_code, _out, err) = run(&sub, &["check", "."]);
    assert!(!err.contains("not a project"), "{err}");
}

/// A workspace root, whose members each have a `bynk.toml`, is unaffected: each
/// member runs under its header.
#[test]
fn a_workspace_root_still_runs_each_member() {
    let root = projects_dir("workspace", &["alpha", "beta"]);
    write(&root.join("bynk.toml"), "[workspace]\nmembers = [\"*\"]\n");
    let (code, out, err) = run(&root, &["check", "."]);
    assert_eq!(code, 0, "stdout:\n{out}\nstderr:\n{err}");
    let said = format!("{out}{err}");
    assert!(
        said.contains("── alpha ──") && said.contains("── beta ──"),
        "{said}"
    );
    assert!(!err.contains("not a project"), "{err}");
}

/// `fmt` formats a directory of projects as one tree (the repository's own
/// `bynk fmt --check examples` relies on it), each file by its own project.
#[test]
fn fmt_still_formats_a_directory_of_projects() {
    let root = projects_dir("fmt", &["alpha", "beta"]);
    let (code, _out, err) = run(&root, &["fmt", "--check", "."]);
    assert_eq!(code, 0, "stderr:\n{err}");
    // And it does reach the nested files: make one non-canonical.
    write(
        &root.join("beta").join("src").join("beta.bynk"),
        "commons beta {  fn   dbl(n:Int)->Int{n+n}  }\n",
    );
    let (code, _out, err) = run(&root, &["fmt", "--check", "."]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(err.contains("beta.bynk"), "{err}");
}

/// `fmt` inside a project that holds a nested project formats the nested
/// project's files too, as it always has: it is unaffected by the boundary rule
/// that `check` and `test` follow.
#[test]
fn fmt_inside_a_project_still_reaches_a_nested_project() {
    let root = scratch("fmt-inside");
    write(
        &root.join("bynk.toml"),
        "[project]\nname = \"outer\"\n\n[paths]\ninclude = [\".\"]\n",
    );
    project(&root.join("vendor"), "inner");
    write(
        &root
            .join("vendor")
            .join("inner")
            .join("src")
            .join("inner.bynk"),
        "commons inner {  fn   dbl(n:Int)->Int{n+n}  }\n",
    );
    let (code, _out, err) = run(&root, &["fmt", "--check", "."]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(err.contains("inner.bynk"), "{err}");
}

/// The same inside a workspace member.
#[test]
fn fmt_in_a_workspace_member_still_reaches_a_nested_project() {
    let root = projects_dir("fmt-member", &["alpha"]);
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"alpha\"]\n",
    );
    write(
        &root.join("alpha").join("bynk.toml"),
        "[project]\nname = \"alpha\"\n\n[paths]\ninclude = [\".\"]\n",
    );
    project(&root.join("alpha").join("vendor"), "inner");
    write(
        &root
            .join("alpha")
            .join("vendor")
            .join("inner")
            .join("src")
            .join("inner.bynk"),
        "commons inner {  fn   dbl(n:Int)->Int{n+n}  }\n",
    );
    let (code, _out, err) = run(&root, &["fmt", "--check", "."]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(err.contains("inner.bynk"), "{err}");
}

/// `compile` shares `check`'s rooting, so it refuses a directory of projects too.
#[test]
fn compile_refuses_a_directory_of_projects() {
    let root = projects_dir("compile", &["alpha", "beta"]);
    let (code, _out, err) = run(&root, &["compile", ".", "--output", "out-compile"]);
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(
        err.contains("not a project") && err.contains("`alpha`"),
        "{err}"
    );
}
