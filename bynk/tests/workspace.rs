//! Workspaces (packaging slice 1, #2207, decision C): a virtual workspace root
//! runs `bynk check`, `fmt` and `test` across its members and refuses `dev`
//! and `deploy`.
//!
//! Pinned here, over the real binary:
//!  - members are the `members` globs minus `exclude`, in a stable order;
//!  - `rich` heads each member's report; `short` adds no headers, and its
//!    paths are relative to the directory the command ran in;
//!  - every member runs, and the run fails if any does;
//!  - a nested project no pattern lists is not a member;
//!  - a member's own `[registries]` or `[workspace]` is refused;
//!  - an empty workspace is refused;
//!  - a malformed unrelated parent manifest is ignored.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

/// The sibling `bynkc`, present only when the whole workspace was built.
fn bynkc_sibling() -> Option<PathBuf> {
    let p = bynk().parent().unwrap().join("bynkc");
    p.exists().then_some(p)
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("workspace")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Run `bin <args>` in `cwd`, with `env`, capturing (code, stdout, stderr).
fn run(bin: &Path, cwd: &Path, args: &[&str], env: &[(&str, &Path)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .env_remove("BYNK_BYNKC")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn bynk_in(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    run(&bynk(), cwd, args, &[])
}

/// A clean commons `name`, canonically formatted.
fn clean(name: &str) -> String {
    format!("commons {name} {{\n\tfn dbl(n: Int) -> Int {{\n\t\tn + n\n\t}}\n}}\n")
}

/// A commons `name` with a type error at 2:31, which isn't canonically
/// formatted either.
fn broken(name: &str) -> String {
    format!("commons {name} {{\n  fn dbl(n: Int) -> Int {{ n + true }}\n}}\n")
}

/// A member at `root/rel` named `acme/<name>`, holding one commons.
fn member(root: &Path, rel: &str, name: &str, source: &str) {
    write(
        &root.join(rel).join("bynk.toml"),
        &format!("[package]\nname = \"acme/{name}\"\n"),
    );
    write(&root.join(rel).join(format!("src/{name}.bynk")), source);
}

/// A workspace with `packages/*` minus `packages/scratch`: `fx` and `money`
/// are broken, `ok` is clean, and `scratch` (excluded) and `vendor/tool` (no
/// pattern lists it) are broken too, so any report naming them is wrong.
fn shop(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/scratch\"]\n",
    );
    member(&root, "packages/money", "money", &broken("money"));
    member(&root, "packages/fx", "fx", &broken("fx"));
    member(&root, "packages/ok", "ok", &clean("ok"));
    member(&root, "packages/scratch", "scratch", &broken("scratch"));
    member(&root, "vendor/tool", "tool", &broken("tool"));
    root
}

#[test]
fn check_rich_heads_each_member_and_fails_if_any_fails() {
    let root = shop("check-rich");
    let (code, _, err) = bynk_in(&root, &["check"]);
    assert_eq!(code, 1, "{err}");
    let headers: Vec<&str> = err.lines().filter(|l| l.starts_with("── ")).collect();
    assert_eq!(
        headers,
        [
            "── packages/fx ──",
            "── packages/money ──",
            "── packages/ok ──"
        ],
        "every member runs, in order; excluded and unlisted projects don't:\n{err}"
    );
    assert!(err.contains("packages/fx/src/fx.bynk:2:31"), "{err}");
    assert!(err.contains("packages/money/src/money.bynk:2:31"), "{err}");
    assert!(!err.contains("scratch") && !err.contains("tool"), "{err}");
}

#[test]
fn check_short_has_no_headers_and_root_relative_paths() {
    let root = shop("check-short");
    let (code, _, err) = bynk_in(&root, &["check", "--format", "short"]);
    assert_eq!(code, 1, "{err}");
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(
        lines,
        [
            "packages/fx/src/fx.bynk:2:31: error[bynk.types.type_mismatch]: operator `+` requires `Int` or `Float` operands; right operand has type `Bool`",
            "packages/money/src/money.bynk:2:31: error[bynk.types.type_mismatch]: operator `+` requires `Int` or `Float` operands; right operand has type `Bool`",
        ],
        "{err}"
    );
}

#[test]
fn a_clean_workspace_checks_clean() {
    let root = scratch("clean");
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\n",
    );
    member(&root, "a", "a", &clean("a"));
    member(&root, "b", "b", &clean("b"));
    let (code, _, err) = bynk_in(&root, &["check", "--format", "short"]);
    assert_eq!((code, err.as_str()), (0, ""));
}

#[test]
fn a_member_checks_as_its_own_project() {
    let root = shop("member");
    let (code, _, err) = bynk_in(&root.join("packages/ok"), &["check"]);
    assert_eq!(code, 0, "{err}");
    // An unlisted project nested under the root is its own project too.
    let (code, _, err) = bynk_in(&root.join("vendor/tool"), &["check", "--format", "short"]);
    assert_eq!(code, 1);
    assert!(err.starts_with("src/tool.bynk:2:"), "{err}");
}

#[test]
fn a_members_registries_and_workspace_are_refused() {
    let root = shop("member-tables");
    write(
        &root.join("packages/ok/bynk.toml"),
        "[package]\nname = \"acme/ok\"\n\n[registries.acme]\nindex = \"https://x\"\n",
    );
    let (code, _, err) = bynk_in(&root.join("packages/ok"), &["check"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("`[registries]` belongs in the workspace root's `bynk.toml`")
            && err.contains("not a member's"),
        "{err}"
    );
    // And from the root, naming the member.
    let (_, _, err) = bynk_in(&root, &["check", "--format", "short"]);
    assert!(
        err.contains("bynk: packages/ok: `[registries]` belongs in the workspace root's"),
        "{err}"
    );

    write(
        &root.join("packages/ok/bynk.toml"),
        "[workspace]\nmembers = [\"inner\"]\n",
    );
    let (code, _, err) = bynk_in(&root.join("packages/ok"), &["check"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("so it can't declare a `[workspace]` of its own"),
        "{err}"
    );
}

#[test]
fn an_unlisted_nested_project_keeps_its_registries() {
    let root = shop("unlisted");
    write(
        &root.join("vendor/tool/bynk.toml"),
        "[project]\nname = \"tool\"\n\n[registries.acme]\nindex = \"https://x\"\n",
    );
    write(&root.join("vendor/tool/src/tool.bynk"), &clean("tool"));
    let (code, _, err) = bynk_in(&root.join("vendor/tool"), &["check"]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn an_empty_workspace_is_refused() {
    let root = scratch("empty");
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    );
    let (code, _, err) = bynk_in(&root, &["check"]);
    assert_eq!(code, 1);
    assert_eq!(
        err,
        "bynk: bynk.toml: `[workspace]` matches no member: no directory under the root matches `packages/*`\n"
    );
}

#[test]
fn a_malformed_unrelated_parent_is_ignored() {
    let root = scratch("malformed-parent");
    write(&root.join("bynk.toml"), "[workspace\nmembers = [\"app\"]\n");
    write(
        &root.join("app/bynk.toml"),
        "[project]\nname = \"app\"\n\n[registries.acme]\nindex = \"https://x\"\n",
    );
    write(&root.join("app/src/app.bynk"), &clean("app"));
    let (code, _, err) = bynk_in(&root.join("app"), &["check"]);
    assert_eq!(code, 0, "a broken parent counts as absent: {err}");
}

#[test]
fn fmt_runs_across_members_with_root_relative_paths() {
    let root = shop("fmt");
    let (code, _, err) = bynk_in(&root, &["fmt", "--check", "."]);
    assert_eq!(code, 1, "{err}");
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(
        lines,
        [
            "bynk fmt: packages/fx/src/fx.bynk is not canonically formatted",
            "bynk fmt: packages/money/src/money.bynk is not canonically formatted",
        ],
        "{err}"
    );
}

#[test]
fn dev_and_deploy_refuse_at_a_workspace_root() {
    let root = shop("dev-deploy");
    for command in ["dev", "deploy"] {
        let (code, _, err) = bynk_in(&root, &[command]);
        assert_eq!(code, 1, "{command}");
        assert!(
            err.contains("is a workspace root, not a project")
                && err.contains(&format!("run `bynk {command}` inside a member")),
            "{command}: {err}"
        );
    }
}

#[test]
fn test_runs_across_members_under_headers() {
    let Some(bynkc) = bynkc_sibling() else {
        eprintln!("skipping: sibling bynkc not built");
        return;
    };
    let root = shop("test");
    let env = [("BYNK_BYNKC", bynkc.as_path())];
    let (code, out, err) = run(&bynk(), &root, &["test", "--no-run"], &env);
    assert_eq!(code, 1, "fx and money don't compile:\n{out}\n{err}");
    let headers: Vec<&str> = out.lines().filter(|l| l.starts_with("── ")).collect();
    assert_eq!(
        headers,
        [
            "── packages/fx ──",
            "── packages/money ──",
            "── packages/ok ──"
        ],
        "{out}"
    );
    let (code, _, err) = run(&bynk(), &root, &["test", "--format", "json"], &env);
    assert_eq!(code, 1);
    assert!(
        err.contains(
            "bynk test: `--format json` reports one project — run it inside a member of this workspace"
        ),
        "{err}"
    );
    let (code, _, err) = run(&bynk(), &root, &["test", "--format", "ndjson"], &env);
    assert_eq!(code, 1);
    assert!(
        err.contains(
            "bynk test: `--format ndjson` streams one project — run it inside a member of this workspace"
        ),
        "{err}"
    );
}

/// A member holding one commons `name` and a suite `name` with one case
/// `case-<name>`.
fn tested_member(root: &Path, name: &str) {
    member(
        root,
        &format!("packages/{name}"),
        name,
        &format!("commons {name}\n\nfn id(n: Int) -> Int {{\n  n\n}}\n"),
    );
    write(
        &root.join(format!("packages/{name}/src/tests/{name}.test.bynk")),
        &format!("suite {name} {{\n  case \"case-{name}\" {{\n    expect id(1) == 1\n  }}\n}}\n"),
    );
}

fn have_test_runner() -> bool {
    let has = |tool: &str| {
        Command::new(if cfg!(windows) { "where" } else { "which" })
            .arg(tool)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    has("tsx") || (has("tsc") && has("node"))
}

/// #2273: at a workspace root a `--suite` or `--case` value may live in one
/// member only. Every member runs as part of the selection, and each value is
/// judged against all the members' suites, not against each member alone.
#[test]
fn test_filters_are_judged_across_the_members() {
    let Some(bynkc) = bynkc_sibling() else {
        eprintln!("skipping: sibling bynkc not built");
        return;
    };
    if cfg!(windows) || !have_test_runner() {
        eprintln!("skipping: needs a TypeScript runner on a Unix host");
        return;
    }
    let root = scratch("test-filter");
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    );
    tested_member(&root, "a");
    tested_member(&root, "b");
    let env = [("BYNK_BYNKC", bynkc.as_path())];

    // A suite in one member: the other member runs nothing, and the run passes.
    let (code, out, err) = run(&bynk(), &root, &["test", "--suite", "a"], &env);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("✓ case-a") && !out.contains("case-b"), "{out}");

    // A case in the other member, and both at once.
    let (code, out, err) = run(
        &bynk(),
        &root,
        &["test", "--case", "case-b", "--case", "case-a"],
        &env,
    );
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("✓ case-a") && out.contains("✓ case-b"),
        "{out}"
    );

    // A value no member has fails the run before any member runs.
    let (code, out, err) = run(&bynk(), &root, &["test", "--case", "typo"], &env);
    assert_eq!(code, 1);
    assert!(
        err.contains("bynk test: no case matched --case \"typo\""),
        "{err}"
    );
    assert!(!out.contains("── "), "no member ran:\n{out}");

    // `--suite` and `--case` combine as and across the members too.
    let (code, _, err) = run(
        &bynk(),
        &root,
        &["test", "--suite", "a", "--case", "case-b"],
        &env,
    );
    assert_eq!(code, 1);
    assert!(err.contains("no case matched --case \"case-b\""), "{err}");
}

#[test]
fn bynkc_refuses_to_build_a_workspace_root() {
    let Some(bynkc) = bynkc_sibling() else {
        eprintln!("skipping: sibling bynkc not built");
        return;
    };
    let root = shop("bynkc-compile");
    let (code, _, err) = run(&bynkc, &root, &["compile", ".", "-o", "out"], &[]);
    assert_eq!(code, 1);
    assert!(
        err.contains("this is a workspace root, not a project"),
        "{err}"
    );
}
