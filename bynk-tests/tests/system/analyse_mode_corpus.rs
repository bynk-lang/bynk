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

/// A failure line for `input`, or `None` when `bynk check` succeeded cleanly of internal errors.
fn check(input: &Path) -> Option<String> {
    // A project with dependencies gets its `bynk.lock` written beside `bynk.toml`; the corpus is
    // checked in place, so remove a lock this run created rather than leave the tree dirty.
    let lock = input.join("bynk.lock");
    let had_lock = input.is_dir() && lock.exists();
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("check")
        .arg(input)
        .output()
        .expect("run bynk check");
    if input.is_dir() && !had_lock {
        let _ = std::fs::remove_file(&lock);
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
    let dir = std::env::temp_dir().join(format!("bynk-4238-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let broken = dir.join("broken.bynk");
    std::fs::write(&broken, "commons demo\n\nfn f() -> Int {\n  \"x\"\n}\n").unwrap();
    let failure = check(&broken).expect("a type error must fail the check");
    assert!(failure.contains("exited"), "{failure}");
    let _ = std::fs::remove_dir_all(&dir);
}
