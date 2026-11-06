//! #1758: `bynkc test` spawns each runner by the path detection resolved, and a
//! runner that is found but will not start is reported as such, not as missing.
//!
//! The Windows failure was a `tsc.cmd` shim that detection found (through
//! `PATHEXT`) but a bare-name spawn never started, so every runner was skipped
//! and the run advised installing `tsc`. The Windows test below pins the fix
//! where it can be seen: CI's Windows leg installs `typescript@5` with npm and
//! sets `BYNK_REQUIRE_TSC`. The Unix test pins the reporting half: a `tsc` on
//! `PATH` that is executable, so detection finds it, but whose interpreter does
//! not exist, so spawning it fails.

#[cfg(windows)]
#[path = "../require/mod.rs"]
mod require;

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A one-commons project with one passing case.
fn one_case_project(project: &Path) {
    std::fs::create_dir_all(project.join("src/demo")).unwrap();
    std::fs::write(project.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(
        project.join("src/demo/m.bynk"),
        "commons demo.m\n\nfn one() -> Int { 1 }\n",
    )
    .unwrap();
    std::fs::write(
        project.join("src/demo/m_test.bynk"),
        "suite demo.m {\n  case \"one\" {\n    expect one() == 1\n  }\n}\n",
    )
    .unwrap();
}

/// #1758 acceptance: on Windows an npm-installed `tsc` is a `tsc.cmd` shim,
/// which `bynkc test` used to find but never start. Runs whenever `tsc`
/// resolves; under `BYNK_REQUIRE_TSC` (set on CI, which installs
/// `typescript@5` with npm) a missing `tsc` fails rather than skips.
#[cfg(windows)]
#[test]
fn an_npm_installed_tsc_shim_runs_the_suite() {
    if which::which("tsc").is_err() {
        assert!(
            !require::is_required("BYNK_REQUIRE_TSC"),
            "BYNK_REQUIRE_TSC is set but `tsc` is not on PATH"
        );
        eprintln!("skipping: `tsc` is not on PATH");
        return;
    }
    let project = scratch("runner-windows-shim").join("project");
    one_case_project(&project);
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["test", "."])
        .current_dir(&project)
        .output()
        .expect("bynkc runs");
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `bynkc test <extra>` on the one-case project with only a `tsc` on
/// `PATH` that is executable, so detection finds it, but whose interpreter
/// does not exist, so spawning it fails. Returns the path of that `tsc` and
/// the run's output.
#[cfg(unix)]
fn run_with_unstartable_tsc(tag: &str, extra: &[&str]) -> (PathBuf, std::process::Output) {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch(tag);
    let project = dir.join("project");
    one_case_project(&project);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let tsc = bin.join("tsc");
    std::fs::write(&tsc, "#!/nonexistent/bynk-test-interpreter\n").unwrap();
    std::fs::set_permissions(&tsc, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .args(extra)
        .arg(".")
        .current_dir(&project)
        .env("PATH", &bin)
        .output()
        .expect("bynkc runs");
    (tsc, out)
}

#[cfg(unix)]
#[test]
fn a_runner_that_is_found_but_will_not_start_is_reported_not_called_missing() {
    let (tsc, out) = run_with_unstartable_tsc("runner-will-not-start", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("no test runner could be started"),
        "a found runner that fails to start must be reported; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "`{}` was found but could not be started",
            tsc.display()
        )),
        "the report names the resolved path; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("npm install -g tsx"),
        "the install advice still follows; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("requires either"),
        "an installed runner must not be reported as missing; stderr:\n{stderr}"
    );
}

/// #1761 review: `--coverage` returned its own "requires `tsc`" message before
/// the start failures were reported.
#[cfg(unix)]
#[test]
fn coverage_reports_a_tsc_that_will_not_start_not_a_missing_one() {
    let (tsc, out) = run_with_unstartable_tsc("runner-will-not-start-coverage", &["--coverage"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains(&format!(
            "`{}` was found but could not be started",
            tsc.display()
        )),
        "stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("requires `tsc` and `node` on PATH"),
        "an installed tsc must not be reported as missing; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("npm install -g tsx"),
        "`--coverage` doesn't accept tsx, so it isn't advised; stderr:\n{stderr}"
    );
}

/// The JSON document carries the same report: a `runtime` error whose
/// `stderr` names each runner that would not start.
#[cfg(unix)]
#[test]
fn the_json_document_reports_a_runner_that_will_not_start() {
    let (tsc, out) = run_with_unstartable_tsc("runner-will-not-start-json", &["--format", "json"]);
    assert!(!out.status.success());
    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is the JSON document");
    assert_eq!(doc["error"]["kind"], "runtime", "{doc}");
    assert_eq!(
        doc["error"]["message"], "no test runner could be started",
        "{doc}"
    );
    assert!(
        doc["error"]["stderr"]
            .as_str()
            .is_some_and(|e| e.contains(&tsc.display().to_string())),
        "{doc}"
    );
}
