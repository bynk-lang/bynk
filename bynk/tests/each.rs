//! #3274: `bynk check --each` and `bynk test --each` run each project directly
//! inside a directory that has no `bynk.toml`, as they run each member at a
//! `[workspace]` root: a header per project, every project run, a non-zero exit
//! if any fails. The flag is opt-in; without it the directory is still refused
//! (#3248), and the refusal now names the flag.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("each-{name}"));
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

const GOOD: &str = "commons shared\n\nfn dbl(n: Int) -> Int {\n  n + n\n}\n";
const BAD: &str = "commons shared\n\nfn dbl(n: Int) -> Int {\n  \"not an int\"\n}\n";

/// A project named `name` under `root`, whose one unit is `source`.
fn project_with(root: &Path, name: &str, source: &str) {
    write(
        &root.join(name).join("bynk.toml"),
        &format!("[project]\nname = \"{name}\"\n"),
    );
    write(&root.join(name).join("src/shared.bynk"), source);
}

fn project(root: &Path, name: &str) {
    project_with(root, name, GOOD);
}

/// A directory with no manifest holding `names` as passing projects.
fn projects_dir(test: &str, names: &[&str]) -> PathBuf {
    let root = scratch(test);
    for name in names {
        project(&root, name);
    }
    root
}

fn headers(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with("── ")).collect()
}

/// (1) Every project runs under its header, in name order, and the run passes.
#[test]
fn check_each_runs_every_project_under_headers() {
    let root = projects_dir("pass", &["gamma", "alpha", "beta"]);
    let (code, out, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert_eq!(
        headers(&err),
        ["── alpha ──", "── beta ──", "── gamma ──"],
        "{err}"
    );
    assert!(!err.contains("not a project"), "{err}");
}

/// (2) A failing project does not stop the others, and fails the run.
#[test]
fn a_failing_project_fails_the_run_after_the_rest_ran() {
    let root = projects_dir("fail", &["alpha", "gamma"]);
    project_with(&root, "beta", BAD);
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(
        headers(&err),
        ["── alpha ──", "── beta ──", "── gamma ──"],
        "every project ran: {err}"
    );
}

/// (3) A project whose manifest is malformed is reported under its header, and
/// the run carries on.
#[test]
fn a_malformed_manifest_is_reported_under_its_header() {
    let root = projects_dir("malformed", &["alpha", "gamma"]);
    write(&root.join("beta/bynk.toml"), "[project\nname = ");
    write(&root.join("beta/src/shared.bynk"), GOOD);
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 1, "{err}");
    let beta = err.find("── beta ──").expect("beta has a header");
    let gamma = err.find("── gamma ──").expect("gamma still ran");
    assert!(beta < gamma && err[beta..gamma].contains("beta"), "{err}");
}

/// (4) A subdirectory with `.bynk` files but no manifest is skipped with a
/// note; one with none is skipped silently.
#[test]
fn a_manifestless_subdirectory_is_skipped_with_a_note() {
    let root = projects_dir("skipped", &["alpha", "beta"]);
    write(&root.join("docs/notes.bynk"), "commons notes\n");
    write(&root.join("scripts/run.sh"), "echo hi\n");
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("bynk: note: skipped `docs`: it has no bynk.toml, so it is not a project"),
        "{err}"
    );
    assert!(!err.contains("scripts"), "{err}");
    assert_eq!(headers(&err), ["── alpha ──", "── beta ──"], "{err}");
}

/// (5) Projects only two levels down: `--each` says so and runs nothing, and
/// pointing it at the directory that holds them works.
#[test]
fn projects_only_below_one_level_are_named_not_reached() {
    let root = scratch("deep");
    project(&root.join("group"), "alpha");
    project(&root.join("group"), "beta");
    let (code, out, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(
        err.contains("holds no project directly inside it")
            && err.contains("`group/alpha`")
            && err.contains("`group/beta`")
            && err.contains("pass the directory that holds them"),
        "{err}"
    );
    assert!(headers(&err).is_empty(), "nothing ran: {err}");
    let (code, _, err) = run(&root, &["check", "--each", "group"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        headers(&err),
        ["── group/alpha ──", "── group/beta ──"],
        "{err}"
    );
}

/// (6) A project nested inside a member belongs to that member's boundary: it is
/// not run as a project of its own.
#[test]
fn a_project_inside_a_member_is_not_run_separately() {
    let root = projects_dir("inner", &["alpha"]);
    project(&root.join("alpha/vendor"), "inner");
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(headers(&err), ["── alpha ──"], "{err}");
    assert!(!err.contains("── alpha/vendor"), "{err}");
}

/// (7) The directories discovery never enters are never members.
#[test]
fn caches_and_hidden_directories_are_not_members() {
    let root = projects_dir("ignored", &["alpha"]);
    for hidden in ["out", "node_modules", ".hidden"] {
        project_with(&root, hidden, BAD);
    }
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(headers(&err), ["── alpha ──"], "{err}");
}

/// (8) No project at all is an error, not a quiet success.
#[test]
fn a_directory_with_no_project_is_an_error() {
    let root = scratch("empty");
    write(&root.join("notes/readme.txt"), "nothing here\n");
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("holds no project"), "{err}");
    let (code, _, err) = run(&root, &["test", "--each"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("bynk test:") && err.contains("holds no project"),
        "{err}"
    );
}

/// (9) `.bynk` files directly in the directory would be read by no project, as
/// at a workspace root, so they are refused.
#[test]
fn sources_in_the_directory_itself_are_refused() {
    let root = projects_dir("rootsrc", &["alpha"]);
    write(&root.join("loose.bynk"), "commons loose\n");
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("holds `.bynk` files of its own"), "{err}");
    assert!(headers(&err).is_empty(), "{err}");
}

/// (10) `--each` is for a directory of projects. At a project, or inside one,
/// or on a file, it is an error that says so; at a real workspace root it
/// changes nothing.
#[test]
fn each_is_an_error_at_a_project_inside_one_and_on_a_file() {
    let root = projects_dir("misplaced", &["alpha"]);
    let alpha = root.join("alpha");
    let (code, _, err) = run(&alpha, &["check", "--each"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("`--each` applies to a directory of projects, but `.` is a project"),
        "{err}"
    );
    // A manifest-less directory inside a project.
    write(
        &alpha.join("scratch/other/bynk.toml"),
        "[project]\nname = \"o\"\n",
    );
    let (code, _, err) = run(&alpha.join("scratch"), &["check", "--each"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("is inside a project"), "{err}");
    let (code, _, err) = run(&root, &["check", "--each", "alpha/src/shared.bynk"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("takes a directory of projects"), "{err}");
    let (code, _, err) = run(&alpha, &["test", "--each", "--no-run"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("bynk test: `--each` applies to a directory"),
        "{err}"
    );
}

#[test]
fn each_at_a_workspace_root_changes_nothing() {
    let root = projects_dir("wsroot", &["alpha", "beta"]);
    write(&root.join("bynk.toml"), "[workspace]\nmembers = [\"*\"]\n");
    let plain = run(&root, &["check"]);
    let each = run(&root, &["check", "--each"]);
    assert_eq!(plain, each);
    assert_eq!(each.0, 0, "{}", each.2);
    assert_eq!(headers(&each.2), ["── alpha ──", "── beta ──"]);
}

/// (11) A sibling path dependency resolves as it does when the project is run
/// alone: the implicit workspace gives it no more and no less.
#[test]
fn a_sibling_path_dependency_works_as_when_run_alone() {
    let root = scratch("pathdep");
    write(
        &root.join("money/bynk.toml"),
        "[package]\nname = \"acme-labs/money\"\nversion = \"1.0.0\"\nexports = [\"currency\"]\n",
    );
    write(
        &root.join("money/src/currency.bynk"),
        "commons currency\n\nfn double(c: Int) -> Int {\n  c + c\n}\n",
    );
    write(
        &root.join("shop/bynk.toml"),
        "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme-labs/money\", path = \"../money\" }\n",
    );
    write(
        &root.join("shop/src/pricing.bynk"),
        "commons pricing\n\nuses money.currency\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n",
    );
    let alone = run(&root.join("shop"), &["check"]);
    assert_eq!(alone.0, 0, "{}", alone.2);
    let (code, _, err) = run(&root, &["check", "--each"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(headers(&err), ["── money ──", "── shop ──"], "{err}");
}

/// (12) `test --each` refuses what one project's output would have to be, as a
/// workspace root does, and heads each project on stdout.
#[test]
fn test_each_refuses_single_project_flags_and_heads_each_project() {
    let root = projects_dir("test", &["alpha", "beta"]);
    for (flag, said) in [
        (
            vec!["--format", "json"],
            "`--format json` reports one project — run it inside a member of this workspace",
        ),
        (
            vec!["--format", "ndjson"],
            "`--format ndjson` streams one project",
        ),
        (
            vec!["--output", "somewhere"],
            "`--output` names one project's output",
        ),
        (vec!["--mutate"], "`--mutate` mutates one project"),
        (
            vec!["--against", "dev"],
            "`--against` runs one project's suites",
        ),
    ] {
        let mut args = vec!["test", "--each"];
        args.extend(flag);
        let (code, _, err) = run(&root, &args);
        assert_eq!(code, 1, "{err}");
        assert!(err.contains(said), "{said}\n{err}");
    }
    let (code, out, err) = run(&root, &["test", "--each", "--no-run"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(headers(&out), ["── alpha ──", "── beta ──"], "{out}");
}

/// (14) The refusal without the flag names it for `check` and `test`; `compile`
/// cannot take it, so its refusal does not, and the flag is not accepted there.
#[test]
fn the_refusal_names_each_for_check_and_test_but_not_compile() {
    let root = projects_dir("refusal", &["alpha", "beta"]);
    for cmd in ["check", "test"] {
        let (code, _, err) = run(&root, &[cmd]);
        assert_eq!(code, 1, "{err}");
        assert!(
            err.contains("pass `--each` to run it over each")
                && err.contains("to keep that choice in the repository")
                && err.contains("members = [\"*\"]"),
            "{cmd}: {err}"
        );
    }
    let (code, _, err) = run(&root, &["compile", ".", "-o", "out"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("not a project")
            && !err.contains("--each")
            && err.contains("to run it over each"),
        "{err}"
    );
    let (code, _, err) = run(&root, &["compile", ".", "-o", "out", "--each"]);
    assert_ne!(code, 0);
    assert!(err.contains("unexpected argument '--each'"), "{err}");
}

/// (15) An implicit workspace is `[workspace] members = ["*"]`: the same
/// projects give the same reports and the same exit code either way.
#[test]
fn each_matches_a_root_workspace_manifest() {
    let root = projects_dir("equivalence", &["alpha", "gamma"]);
    project_with(&root, "beta", BAD);
    let implicit = run(&root, &["check", "--each"]);
    write(&root.join("bynk.toml"), "[workspace]\nmembers = [\"*\"]\n");
    let real = run(&root, &["check"]);
    assert_eq!(implicit.0, 1);
    assert_eq!(implicit, real);
}
