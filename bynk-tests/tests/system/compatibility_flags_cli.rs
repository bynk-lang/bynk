//! #1890: the `bynk.project.duplicate_compatibility_flag` warning, as the real
//! `bynk` CLI prints it. The fixture `1890_workers_compatibility_flags` has
//! two contexts (so two Workers) and repeats one flag in `[workers]
//! compatibility_flags`. Both `check` and `compile` must report the repeat,
//! once for the build rather than once per Worker, attributed to the
//! root-relative `bynk.toml` rather than a path joined onto however the
//! project root was typed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CODE: &str = "bynk.project.duplicate_compatibility_flag";
const MESSAGE: &str =
    "`bynk.toml`'s `[workers] compatibility_flags` lists `nodejs_compat` more than once";

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/1890_workers_compatibility_flags")
}

/// Run `bynk` with `args`, returning success and stdout + stderr.
fn bynk(args: &[&std::ffi::OsStr]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(args)
        .output()
        .expect("run bynk");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

/// A scratch copy of the fixture: `bynk compile` writes a schema lock into
/// the project it compiles, and the committed fixture must stay untouched.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}

#[test]
fn check_reports_the_duplicate_once() {
    let (ok, out) = bynk(&["check".as_ref(), fixture().as_os_str()]);
    assert!(ok, "a warning must not fail `check`:\n{out}");
    assert_eq!(
        out.matches(CODE).count(),
        1,
        "one warning per build:\n{out}"
    );
    assert!(
        out.contains(&format!("warning[{CODE}]: {MESSAGE}")),
        "{out}"
    );
}

#[test]
fn compile_reports_the_duplicate_once_against_bynk_toml() {
    let dir = Scratch(
        std::env::temp_dir().join(format!("bynk_1890_compat_flags_cli_{}", std::process::id())),
    );
    let project = dir.0.join("project");
    copy_dir(&fixture(), &project);
    let output = dir.0.join("out");
    let (ok, out) = bynk(&[
        "compile".as_ref(),
        project.as_os_str(),
        "--output".as_ref(),
        output.as_os_str(),
        "--target".as_ref(),
        "workers".as_ref(),
    ]);
    assert!(ok, "a warning must not fail `compile`:\n{out}");
    assert_eq!(
        out.matches(CODE).count(),
        1,
        "one warning per build:\n{out}"
    );
    // The root-relative identity path, not `<project root>/bynk.toml`.
    assert!(
        out.lines()
            .any(|l| l == format!("bynk.toml: warning[{CODE}]: {MESSAGE}")),
        "{out}"
    );
    for worker in ["greet", "farewell"] {
        let toml =
            fs::read_to_string(output.join("workers").join(worker).join("wrangler.toml")).unwrap();
        assert!(
            toml.contains(
                "compatibility_flags = [\"global_fetch_strictly_public\", \"nodejs_compat\"]\n"
            ),
            "{worker}:\n{toml}"
        );
    }
}
