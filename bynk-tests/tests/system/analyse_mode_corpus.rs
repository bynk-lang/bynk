//! #4238: `bynk check` (analyse mode) runs over the whole corpus, not only the build path.
//!
//! `bynk check` runs every unit in `Mode::Analyse`, a separate path from the one `bynk test`,
//! `bynk build` and the snapshot harness take. #4233 broke only that path: `bynk check` panicked on
//! any project whose tests delivered an event directly, while the positive fixture that reproduces
//! it (`2052_test_event_address`) passed under `bynk test`. Nothing in CI ran analyse mode over a
//! project with tests, so the org canary found it after the release.
//!
//! This runs the real `bynk check` over every positive fixture and every project under `examples/`.
//! Each must exit successfully (warnings are fine, errors are not) and print no internal error. It
//! needs no TypeScript runner, and is cheap: analyse mode skips emission.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// What `bynk check` is given for a positive fixture: a project's root, a single file, or the
/// `src/` tree of a fixture with no manifest, as the snapshot harness reads each.
fn fixture_input(dir: &Path) -> Option<PathBuf> {
    if dir.join("bynk.toml").is_file() {
        Some(dir.to_path_buf())
    } else if dir.join("input.bynk").is_file() {
        Some(dir.join("input.bynk"))
    } else if dir.join("src").is_dir() {
        Some(dir.join("src"))
    } else {
        None
    }
}

/// Copy a project into `dst`, leaving out its snapshots (`expected/`), which `bynk check` never
/// reads.
fn copy_project(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            if entry.file_name() != "expected" {
                copy_project(&from, &to);
            }
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// The scratch directory for a project's copy, under `CARGO_TARGET_TMPDIR` like the other system
/// tests, named by the project's place in the corpus so two never share one.
fn scratch(input: &Path) -> PathBuf {
    let name: String = input
        .strip_prefix(repo().canonicalize().unwrap_or_else(|_| repo()))
        .unwrap_or(input)
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("analyse_mode_corpus")
        .join(name)
}

/// A failure line for `input`, or `None` when `bynk check` succeeded cleanly of internal errors.
///
/// A project is checked from a scratch copy, not in place: `bynk check` writes a `bynk.lock`
/// beside a manifest that has dependencies (#4310), and an interrupted run would leave one in the
/// tracked corpus, where the next run would then read it instead of writing it. The copy is
/// removed whether or not the check passes. A single file is read, not written, so it is checked
/// where it is.
fn check(input: &Path) -> Option<String> {
    let copy = input.is_dir().then(|| scratch(input));
    let target = match &copy {
        Some(dir) => {
            let _ = std::fs::remove_dir_all(dir);
            copy_project(input, dir);
            dir.as_path()
        }
        None => input,
    };
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("check")
        .arg(target)
        .output()
        .expect("run bynk check");
    if let Some(dir) = &copy {
        let _ = std::fs::remove_dir_all(dir);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let internal = stderr.contains("panicked at") || stderr.contains("internal error");
    if out.status.success() && !internal {
        return None;
    }
    Some(format!(
        "`bynk check {}` {}\n{stdout}{stderr}",
        input.display(),
        match out.status.code() {
            Some(0) => "printed an internal error".to_string(),
            Some(c) => format!("exited {c}"),
            None => "was killed".to_string(),
        }
    ))
}

/// Check every input across the machine's cores, returning each failure.
fn check_all(inputs: Vec<PathBuf>) -> Vec<String> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = inputs.len().div_ceil(threads).max(1);
    let mut failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = inputs
            .chunks(chunk)
            .map(|part| s.spawn(move || part.iter().filter_map(|i| check(i)).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a checking thread panicked"))
            .collect()
    });
    failures.sort();
    failures
}

fn sub_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn bynk_check_passes_over_every_positive_fixture() {
    let positive = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let inputs: Vec<PathBuf> = sub_dirs(&positive)
        .iter()
        .map(|d| fixture_input(d).unwrap_or_else(|| panic!("{} has no input", d.display())))
        .collect();
    assert!(inputs.len() > 100, "found only {} fixtures", inputs.len());
    let failures = check_all(inputs);
    assert!(
        failures.is_empty(),
        "`bynk check` failed on {} positive fixture(s):\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn bynk_check_passes_over_every_example_project() {
    let inputs: Vec<PathBuf> = sub_dirs(&repo().join("examples"));
    assert!(!inputs.is_empty(), "no projects under examples/");
    let failures = check_all(inputs);
    assert!(
        failures.is_empty(),
        "`bynk check` failed on {} example project(s):\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The harness reports a failing input rather than passing it by default.
#[test]
fn a_failing_input_is_reported() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("analyse_mode_corpus_broken");
    std::fs::create_dir_all(&dir).unwrap();
    let broken = dir.join("broken.bynk");
    std::fs::write(&broken, "commons demo\n\nfn f() -> Int {\n  \"x\"\n}\n").unwrap();
    let failure = check(&broken);
    let _ = std::fs::remove_dir_all(&dir);
    let failure = failure.expect("a type error must fail the check");
    assert!(failure.contains("exited"), "{failure}");
}
