//! #3941: `--env <name>` on `bynk check`, `build`, `compile` and `test` builds with
//! that environment's `env(…)` values, so a required variable the environment does
//! not set is `bynk.env.missing` there, found before a deploy finds it.

use std::path::{Path, PathBuf};
use std::process::Output;

#[path = "support/dev_child.rs"]
mod dev_child;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("env_flag")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A project whose `Origin` constant is required: `default` (through `[env.default.vars]`) and
/// `production` set it; `staging` does not, and `[vars]` does not cover it.
fn project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("bynk.toml"),
        "[project]\nname = \"envflag\"\nversion = \"0.1.0\"\n\n[env.default.vars]\nORIGIN = \"https://app\"\n\n[env.production.vars]\nORIGIN = \"https://prod\"\n\n[env.staging.vars]\nOTHER = \"x\"\n",
    );
    write(
        &dir.join("src/config.bynk"),
        "commons config\n\nconst Origin: String = env(ORIGIN)\nconst Other: String = env(OTHER, \"d\")\n",
    );
    dir
}

fn bynk(args: &[&str], dir: &Path) -> Output {
    dev_child::bynk_command()
        .args(args)
        .arg(dir)
        .output()
        .expect("runs bynk")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn check_env_finds_a_required_variable_the_environment_does_not_set() {
    let dir = project("check");
    let default = bynk(&["check"], &dir);
    assert!(default.status.success(), "{}", stderr(&default));
    let production = bynk(&["check", "--env", "production"], &dir);
    assert!(production.status.success(), "{}", stderr(&production));
    let staging = bynk(&["check", "--env", "staging"], &dir);
    assert!(!staging.status.success(), "staging has no ORIGIN");
    let err = stderr(&staging);
    assert!(err.contains("bynk.env.missing"), "{err}");
    assert!(err.contains("`staging`"), "{err}");
}

#[test]
fn compile_and_build_env_resolve_that_environments_values() {
    let dir = project("compile");
    let out = dir.join("out-prod");
    let o = dev_child::bynk_command()
        .args(["compile", "--env", "production", "--output"])
        .arg(&out)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let emitted = std::fs::read_to_string(out.join("config.ts")).expect("config.ts");
    assert!(emitted.contains("https://prod"), "{emitted}");
    assert!(!emitted.contains("https://app"), "{emitted}");
    // `default` is what a build without the flag resolves.
    let out = dir.join("out-default");
    let o = dev_child::bynk_command()
        .args(["compile", "--output"])
        .arg(&out)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let emitted = std::fs::read_to_string(out.join("config.ts")).unwrap();
    assert!(emitted.contains("https://app"), "{emitted}");
    // A named environment with no value fails the build, naming the variable.
    let o = dev_child::bynk_command()
        .args(["build", "--env", "staging", "--out"])
        .arg(dir.join("out-staging"))
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(stderr(&o).contains("bynk.env.missing"), "{}", stderr(&o));
}

#[test]
fn test_env_builds_with_that_environments_values() {
    let dir = project("test");
    let default = bynk(&["test", "--no-run"], &dir);
    assert!(default.status.success(), "{}", stderr(&default));
    let staging = bynk(&["test", "--no-run", "--env", "staging"], &dir);
    assert!(!staging.status.success(), "staging has no ORIGIN");
    let err = format!(
        "{}{}",
        stderr(&staging),
        String::from_utf8_lossy(&staging.stdout)
    );
    assert!(err.contains("bynk.env.missing"), "{err}");
}

#[test]
fn test_env_is_refused_where_the_run_would_ignore_it() {
    let dir = project("refused");
    for mode in [&["--mutate"][..], &["--against", "dev"][..]] {
        let mut args = vec!["test", "--env", "staging"];
        args.extend_from_slice(mode);
        let o = bynk(&args, &dir);
        assert!(!o.status.success(), "{mode:?}");
        let err = stderr(&o);
        assert!(err.contains("cannot be combined with"), "{mode:?}: {err}");
        assert!(err.contains("--env"), "{mode:?}: {err}");
    }
}
