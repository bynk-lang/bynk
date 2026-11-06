//! Behavioural fixtures: positive fixtures whose `suite`s are **run**, not just
//! type-checked. #1660 (runtime-semantics track, slice G0; the track is retired,
//! its summary in `design/archive/retired-tracks.md`).
//!
//! The other positive-fixture gates certify two things about a program. Its
//! emitted TypeScript matches a blessed golden (`e2e.rs`), and it passes
//! `tsc --strict` (`tsc_verify.rs`). Neither runs anything, and a golden
//! blesses whatever was emitted. Fixtures `139_agent_state_zero_option` and
//! `155_state_sum_machine` blessed an agent-state reload fault (#1649) for
//! months, and 71 positive fixtures carried `suite`s that only four tests ever
//! executed.
//!
//! **The marker.** A project-form positive fixture opts in by carrying an
//! `expected_run.txt`:
//!
//! ```text
//! # comments and blank lines are ignored
//! passed=3 failed=1
//! fail <case name>
//! ```
//!
//! - `passed=`/`failed=` are the exact case counts the run must report.
//! - Each `fail <case name>` line names a case that must **fail**. Every other
//!   case must pass.
//!
//! Case names must be unique across a fixture's suites, because a `fail` line
//! names a case by name alone.
//!
//! The check is strict in both directions, like a strict `xfail`. A listed case
//! that starts passing fails this test too, so the slice that fixes a known
//! defect must also delete its `fail` line. Cite the tracking issue in a comment
//! above each line.
//!
//! A suite-bearing fixture without the marker is not executed: the class is
//! opt-in and reviewable.
//!
//! **The run.** Each marked fixture is copied to a scratch directory first,
//! because `bynk test` compiles and may write into its input tree (an events
//! project writes `bynk.schema.lock`). The copy is then driven through the real
//! CLI with `bynk test <root> --output <tmp> --format json`, and the pinned
//! JSON document (`bynk_driver::test_json::TestRun`) is compared against the
//! marker. `<root>` is the fixture directory when it has a `bynk.toml`, and its
//! `src/` otherwise: the same rule `adapter_flattened_capability_stub_behaviour.rs`
//! uses.
//!
//! The run must report **at least one case**. `bynk test` exits 0 with "no test
//! declarations found" on a project with no suites, so an empty run would
//! otherwise pass as green.
//!
//! **Target.** Bundle only, because `bynk test` has no `--target` flag. A
//! fixture with `target.txt = workers` still runs its suites as bundle.
//!
//! **Gating.** Like the other toolchain-driving behavioural tests, this skips on
//! Windows (the CLI's runner detection is Unix-only) and skips loudly when no
//! TypeScript runner is on `PATH`. `BYNK_REQUIRE_TSC` turns the skip into a
//! failure, using the shared `require` contract rather than `.is_ok()`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../require/mod.rs"]
mod require;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";
const MARKER: &str = "expected_run.txt";

/// What a fixture's `expected_run.txt` asserts about its run.
#[derive(Debug, PartialEq)]
struct Expected {
    passed: u64,
    failed: u64,
    /// Case names that must fail; every other case must pass.
    failing: Vec<String>,
}

/// Parse an `expected_run.txt`. Errors name the offending line, so a malformed
/// marker fails with a message rather than a silent mismatch.
fn parse_marker(text: &str) -> Result<Expected, String> {
    let mut counts: Option<(u64, u64)> = None;
    let mut failing = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix("fail ") {
            let name = name.trim();
            if name.is_empty() {
                return Err(format!("line {}: `fail` needs a case name", i + 1));
            }
            failing.push(name.to_string());
            continue;
        }
        if counts.is_some() {
            return Err(format!("line {}: a second counts line: `{line}`", i + 1));
        }
        let mut passed = None;
        let mut failed = None;
        for field in line.split_whitespace() {
            let (key, value) = field
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected `key=value`, got `{field}`", i + 1))?;
            let n: u64 = value
                .parse()
                .map_err(|_| format!("line {}: `{key}` is not a count: `{value}`", i + 1))?;
            match key {
                "passed" => passed = Some(n),
                "failed" => failed = Some(n),
                other => return Err(format!("line {}: unknown key `{other}`", i + 1)),
            }
        }
        match (passed, failed) {
            (Some(p), Some(f)) => counts = Some((p, f)),
            _ => {
                return Err(format!(
                    "line {}: the counts line needs both `passed=` and `failed=`",
                    i + 1
                ));
            }
        }
    }
    let (passed, failed) = counts.ok_or("no `passed=N failed=M` line")?;
    if failing.len() as u64 != failed {
        return Err(format!(
            "`failed={failed}` but {} `fail <case>` line(s): every expected failure must be named",
            failing.len()
        ));
    }
    Ok(Expected {
        passed,
        failed,
        failing,
    })
}

/// One case's name and whether it passed, read from the JSON document.
#[derive(Debug, PartialEq)]
struct CaseOutcome {
    suite: String,
    name: String,
    passed: bool,
}

/// Compare a `bynk test --format json` document against the marker. Returns
/// every discrepancy, not just the first, so one CI run shows the whole picture.
fn compare(doc: &serde_json::Value, expected: &Expected) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(error) = doc.get("error") {
        problems.push(format!("the run did not complete: {error}"));
        return problems;
    }
    let mut cases = Vec::new();
    for suite in doc["suites"].as_array().into_iter().flatten() {
        let suite_name = suite["name"].as_str().unwrap_or("?").to_string();
        for case in suite["cases"].as_array().into_iter().flatten() {
            cases.push(CaseOutcome {
                suite: suite_name.clone(),
                name: case["name"].as_str().unwrap_or("?").to_string(),
                passed: case["outcome"].as_str() == Some("pass"),
            });
        }
    }
    if cases.is_empty() {
        problems
            .push("the run reported no cases (`bynk test` exits 0 when it finds none)".to_string());
        return problems;
    }
    // A `fail <case>` line names a case by name alone, so names must be unique
    // across the run's suites. Otherwise a passing `A / x` and a failing
    // `B / x` collapse into one, and a listed failure can mask an unlisted one
    // whenever the totals happen to agree. (#1683 review.)
    let mut seen: Vec<&CaseOutcome> = Vec::new();
    for c in &cases {
        if let Some(first) = seen.iter().find(|s| s.name == c.name) {
            problems.push(format!(
                "case `{}` appears in suites `{}` and `{}`; behavioural fixtures need unique case names",
                c.name, first.suite, c.suite
            ));
        } else {
            seen.push(c);
        }
    }
    if !problems.is_empty() {
        return problems;
    }
    let passed = doc["passed"].as_u64().unwrap_or(0);
    let failed = doc["failed"].as_u64().unwrap_or(0);
    if (passed, failed) != (expected.passed, expected.failed) {
        problems.push(format!(
            "expected passed={} failed={}, got passed={passed} failed={failed}",
            expected.passed, expected.failed
        ));
    }
    for name in &expected.failing {
        match cases.iter().find(|c| &c.name == name) {
            None => problems.push(format!("`fail {name}`: no case by that name ran")),
            Some(c) if c.passed => problems.push(format!(
                "`{}` / `{name}` now passes. If a fix landed, delete its `fail` line",
                c.suite
            )),
            Some(_) => {}
        }
    }
    for c in &cases {
        if !c.passed && !expected.failing.contains(&c.name) {
            let message = doc["suites"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|s| s["cases"].as_array().into_iter().flatten())
                .find(|k| k["name"].as_str() == Some(c.name.as_str()))
                .and_then(|k| k["message"].as_str())
                .unwrap_or("");
            problems.push(format!(
                "`{}` / `{}` failed unexpectedly: {message}",
                c.suite, c.name
            ));
        }
    }
    problems
}

fn tool_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The same fallback chain `bynk test` walks: `tsx`, or `tsc` + `node`, or `npx`.
fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

/// Every positive fixture carrying the marker, sorted for a stable report.
fn marked_fixtures() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut out: Vec<PathBuf> = fs::read_dir(&root)
        .expect("read positive fixtures")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(MARKER).is_file())
        .collect();
    out.sort();
    out
}

/// Copy a fixture (everything except its `expected/` goldens) into `dest`, so the
/// run never writes into the source tree. Marker files such as `expected_run.txt`
/// and `target.txt` come along too; `bynk test` ignores them.
fn copy_fixture(src: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("create scratch fixture dir");
    for entry in fs::read_dir(src).expect("read fixture dir").flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "expected" {
            continue;
        }
        let target = dest.join(&name);
        if path.is_dir() {
            copy_fixture(&path, &target);
        } else {
            fs::copy(&path, &target).expect("copy fixture file");
        }
    }
}

/// Run one marked fixture and return its discrepancies (empty when it matches).
fn run_fixture(fixture: &Path, scratch: &Path) -> Vec<String> {
    let expected = match fs::read_to_string(fixture.join(MARKER))
        .map_err(|e| e.to_string())
        .and_then(|t| parse_marker(&t))
    {
        Ok(e) => e,
        Err(e) => return vec![format!("{MARKER}: {e}")],
    };

    let name = fixture.file_name().expect("fixture name");
    let copy = scratch.join(name).join("project");
    let _ = fs::remove_dir_all(scratch.join(name));
    copy_fixture(fixture, &copy);
    let input = if copy.join("bynk.toml").is_file() {
        copy.clone()
    } else {
        copy.join("src")
    };
    // Each run gets its own parent directory: the runner writes its executed
    // `out-js` tree as a *sibling* of `--output` (see `property_behaviour.rs`).
    let output = scratch.join(name).join("run").join("out");

    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .arg("--format")
        .arg("json")
        .output()
        .expect("run bynk test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let doc: serde_json::Value = match serde_json::from_str(&stdout) {
        Ok(d) => d,
        Err(e) => {
            return vec![format!(
                "`bynk test --format json` did not print a JSON document ({e}).\nstdout:\n{stdout}\nstderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            )];
        }
    };
    compare(&doc, &expected)
}

#[test]
fn marked_positive_fixtures_behave_as_expected() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if require::is_required(REQUIRE_ENV) {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!(
            "\n!!! BEHAVIOURAL FIXTURES SKIPPED !!!\nno TypeScript runner (tsx or tsc+node) on PATH.\n"
        );
        return;
    }

    let fixtures = marked_fixtures();
    assert!(
        !fixtures.is_empty(),
        "no positive fixture carries `{MARKER}`, so the behavioural gate would certify nothing"
    );
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("behaviour-fixtures");
    // Each fixture runs in its own scratch directory, so the runs are
    // independent. A small worker pool keeps the whole gate to a few seconds
    // per core instead of about 1 s per fixture serially. nextest cannot
    // parallelise this, because it is one test.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Vec<String>>> = fixtures
        .iter()
        .map(|_| std::sync::Mutex::new(Vec::new()))
        .collect();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(fixture) = fixtures.get(i) else {
                        break;
                    };
                    *results[i].lock().unwrap() = run_fixture(fixture, &scratch);
                }
            });
        }
    });

    let mut report = String::new();
    for (fixture, problems) in fixtures.iter().zip(results) {
        let problems = problems.into_inner().unwrap();
        if !problems.is_empty() {
            let name = fixture.file_name().unwrap().to_string_lossy();
            report.push_str(&format!("\n{name}:\n"));
            for p in problems {
                report.push_str(&format!("  - {p}\n"));
            }
        }
    }
    assert!(
        report.is_empty(),
        "behavioural fixtures disagree with their `{MARKER}`:{report}"
    );
}

/// Run a fixture with `needle` (a statement in the emitted verifier's compiled
/// JavaScript) replaced by `void 0`, and return each case's name and whether it
/// passed. A `tsc` shim on `PATH` runs the real `tsc`, then rewrites
/// `out-js/**/auth.js` before `node` runs it, and fails loudly when `needle` is
/// not there, so a refactor of the verifier cannot turn the mutation into a no-op.
fn run_mutated(fixture: &Path, scratch: &Path, tag: &str, needle: &str) -> Vec<(String, bool)> {
    let real_tsc = Command::new("which")
        .arg("tsc")
        .output()
        .expect("which tsc");
    let real_tsc = String::from_utf8_lossy(&real_tsc.stdout).trim().to_string();
    let bin = scratch.join(format!("mutant-bin-{tag}"));
    fs::create_dir_all(&bin).expect("create shim dir");
    let shim = bin.join("tsc");
    fs::write(
        &shim,
        format!(
            r#"#!/bin/sh
"{real_tsc}" "$@" || exit $?
while [ $# -gt 0 ]; do [ "$1" = -p ] && P="$2"; shift; done
OUTJS="$(dirname "$(dirname "$P")")/out-js"
FILES=$(grep -rlF '{needle}' "$OUTJS" --include=auth.js)
[ -n "$FILES" ] || {{ echo "mutation anchor not found: {needle}" >&2; exit 99; }}
for f in $FILES; do sed -i.bak 's/{needle}/void 0/' "$f"; done
"#
        ),
    )
    .expect("write shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod shim");
    }
    let name = format!("{}-{tag}", fixture.file_name().unwrap().to_string_lossy());
    let copy = scratch.join(&name).join("project");
    let _ = fs::remove_dir_all(scratch.join(&name));
    copy_fixture(fixture, &copy);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .env("PATH", path)
        .arg("test")
        .arg(&copy)
        .arg("--output")
        .arg(scratch.join(&name).join("run").join("out"))
        .arg("--format")
        .arg("json")
        .output()
        .expect("run bynk test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "no JSON document ({e}).\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let cases: Vec<(String, bool)> = doc["suites"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["cases"].as_array().into_iter().flatten())
        .map(|c| {
            (
                c["name"].as_str().unwrap_or("?").to_string(),
                c["outcome"].as_str() == Some("pass"),
            )
        })
        .collect();
    assert!(
        !cases.is_empty(),
        "the mutated run reported no cases: {doc}"
    );
    cases
}

/// #2864: `by Forged` and `by Expired` must be able to fail. Each check of the
/// emitted Bearer verifier is turned off in turn, and exactly the cases that
/// rely on it must turn red, while the other principal's case stays green.
#[test]
fn a_lax_bearer_verifier_fails_the_forged_and_expired_cases() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: the `tsc` shim is a POSIX shell script");
        return;
    }
    if !(tool_exists("tsc") && tool_exists("node")) {
        if require::is_required(REQUIRE_ENV) {
            panic!("tsc and node are required, but {REQUIRE_ENV} is set and one is missing");
        }
        eprintln!("\n!!! MUTATION TEST SKIPPED !!!\nit needs `tsc` and `node` on PATH.\n");
        return;
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/2864_system_bearer_refusals");
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("behaviour-fixtures");
    let passed = |cases: &[(String, bool)], name: &str| {
        cases
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no case `{name}` ran"))
            .1
    };

    // The signature check off: a forged token is admitted.
    let lax_sig = run_mutated(&fixture, &scratch, "sig", r#"return Err("bad signature")"#);
    assert!(!passed(&lax_sig, "a forged token is refused"));
    assert!(!passed(&lax_sig, "a forged raw request is refused"));
    assert!(passed(&lax_sig, "an expired token is refused"));
    assert!(passed(&lax_sig, "a missing token is refused"));
    // ... and the inverted claim now holds, so the outcome is read from the seam.
    assert!(passed(&lax_sig, "a forged token is not handled"));

    // The `exp` check off: an expired token is admitted.
    let lax_exp = run_mutated(&fixture, &scratch, "exp", r#"return Err("token expired")"#);
    assert!(!passed(&lax_exp, "an expired token is refused"));
    assert!(!passed(&lax_exp, "an expired raw request is refused"));
    assert!(passed(&lax_exp, "a forged token is refused"));
    assert!(passed(&lax_exp, "a missing token is refused"));
    assert!(passed(&lax_exp, "an expired token is not handled"));
}

#[cfg(test)]
mod marker {
    use super::*;

    #[test]
    fn parses_counts_failures_and_comments() {
        let e = parse_marker("# a comment\n\npassed=2 failed=1\n# #1649\nfail reload reads back\n")
            .unwrap();
        assert_eq!(
            e,
            Expected {
                passed: 2,
                failed: 1,
                failing: vec!["reload reads back".to_string()],
            }
        );
    }

    #[test]
    fn every_expected_failure_must_be_named() {
        assert!(parse_marker("passed=1 failed=1\n").is_err());
        assert!(parse_marker("passed=1 failed=0\nfail x\n").is_err());
    }

    #[test]
    fn rejects_missing_or_malformed_counts() {
        assert!(parse_marker("").is_err());
        assert!(parse_marker("passed=1\n").is_err());
        assert!(parse_marker("passed=one failed=0\n").is_err());
        assert!(parse_marker("passed=1 failed=0 skipped=0\n").is_err());
        assert!(parse_marker("passed=1 failed=0\npassed=1 failed=0\n").is_err());
    }

    fn doc(cases: &[(&str, &str)]) -> serde_json::Value {
        let passed = cases.iter().filter(|(_, o)| *o == "pass").count();
        let failed = cases.len() - passed;
        serde_json::json!({
            "passed": passed,
            "failed": failed,
            "suites": [{
                "name": "s",
                "kind": "suite",
                "cases": cases.iter().map(|(n, o)| serde_json::json!({"name": n, "outcome": o})).collect::<Vec<_>>(),
            }],
        })
    }

    #[test]
    fn a_matching_run_has_no_problems() {
        let e = parse_marker("passed=1 failed=1\nfail b\n").unwrap();
        assert!(compare(&doc(&[("a", "pass"), ("b", "fail")]), &e).is_empty());
    }

    #[test]
    fn a_listed_failure_that_now_passes_is_reported() {
        let e = parse_marker("passed=1 failed=1\nfail b\n").unwrap();
        let problems = compare(&doc(&[("a", "pass"), ("b", "pass")]), &e);
        assert!(
            problems.iter().any(|p| p.contains("now passes")),
            "{problems:?}"
        );
    }

    #[test]
    fn an_unlisted_failure_is_reported() {
        let e = parse_marker("passed=2 failed=0\n").unwrap();
        let problems = compare(&doc(&[("a", "pass"), ("b", "fail")]), &e);
        assert!(
            problems.iter().any(|p| p.contains("failed unexpectedly")),
            "{problems:?}"
        );
    }

    #[test]
    fn duplicate_case_names_across_suites_are_rejected() {
        let e = parse_marker("passed=1 failed=1\nfail x\n").unwrap();
        let doc = serde_json::json!({
            "passed": 1,
            "failed": 1,
            "suites": [
                {"name": "A", "kind": "suite", "cases": [{"name": "x", "outcome": "pass"}]},
                {"name": "B", "kind": "suite", "cases": [{"name": "x", "outcome": "fail"}]},
            ],
        });
        let problems = compare(&doc, &e);
        assert!(
            problems.iter().any(|p| p.contains("unique case names")),
            "{problems:?}"
        );
    }

    #[test]
    fn an_empty_run_is_never_green() {
        let e = parse_marker("passed=0 failed=0\n").unwrap();
        let problems = compare(
            &serde_json::json!({"passed": 0, "failed": 0, "suites": []}),
            &e,
        );
        assert!(
            problems.iter().any(|p| p.contains("no cases")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_run_that_did_not_complete_is_reported() {
        let e = parse_marker("passed=1 failed=0\n").unwrap();
        let problems = compare(
            &serde_json::json!({"passed": 0, "failed": 0, "error": {"kind": "compile"}}),
            &e,
        );
        assert!(
            problems.iter().any(|p| p.contains("did not complete")),
            "{problems:?}"
        );
    }
}
