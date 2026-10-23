//! v0.114 behavioural test for generative `property` tests (testing track slice
//! 2). Snapshots prove the emitted shape; this proves the runtime behaviour: a
//! property that fails over its domain reports a case count, the run's root
//! seed, and a shrunk counterexample with a copy-paste reproduce line — and
//! `--seed <hex>` reproduces the run byte-for-byte.
//!
//! Drives the real `bynkc test` CLI against a fixture project, so it exercises
//! the `--seed` threading, the generator/runner, and shrinking end to end. Like
//! the other toolchain-driving tests it skips loudly when no TypeScript runner
//! (`tsc`+`node` or `tsx`) is available; `BYNK_REQUIRE_TSC=1` turns the skip
//! into a failure (CI).

use std::path::PathBuf;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A TypeScript runner is available if either `tsx` or (`tsc` + `node`) is on
/// PATH — the same fallback chain `bynkc test` itself walks.
fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/property_fail")
}

/// Run `bynkc test <fixture> --seed <seed>` and return combined stdout+stderr.
fn run_with_seed(seed: &str, out_dir: &str) -> String {
    // Emit under the target tmpdir so no build artifacts land in the source tree.
    // Each run gets its own parent dir: the runner derives the executed
    // `out-js` tree as a *sibling* of `--output`, so a shared parent (e.g. two
    // behavioural tests both rooted directly under `CARGO_TARGET_TMPDIR`)
    // would race the emitted `.js` a concurrent test's `tsc`/`node` run reads.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .arg("--seed")
        .arg(seed)
        .output()
        .expect("run bynkc test");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[test]
fn failing_property_reports_seed_and_shrunk_counterexample() {
    // `bynkc test`'s runner detection is Unix-only (it shells out to `which`), so
    // the CLI cannot locate `tsc`/`node`/`tsx` on Windows and never runs the
    // emitted tests. This test drives that CLI end to end, so it is meaningful
    // only where the CLI can run a runner — skip on Windows. (The *emission* is
    // covered platform-independently by the golden fixture `243_property_passes`.)
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    let first = run_with_seed("0x5f3a", "out-a");

    // A domain-wide failure reports the case count, the run's root seed (the one
    // we passed), a shrunk counterexample, and a copy-paste reproduce line.
    assert!(
        first.contains("property failed after"),
        "expected a property-failure banner, got:\n{first}"
    );
    assert!(
        first.contains("(seed 0x5f3a)"),
        "expected the passed root seed in the report, got:\n{first}"
    );
    assert!(
        first.contains("shrunk counterexample:"),
        "expected a shrunk counterexample, got:\n{first}"
    );
    assert!(
        first.contains("--seed 0x5f3a"),
        "expected a reproduce line carrying the root seed, got:\n{first}"
    );

    // Re-running with the same seed reproduces the same counterexample — the
    // whole point of a deterministic, seed-derived generator.
    let second = run_with_seed("0x5f3a", "out-b");
    let extract = |s: &str| {
        s.lines()
            .find(|l| l.contains("shrunk counterexample:"))
            .map(|l| l.trim().to_string())
    };
    assert_eq!(
        extract(&first),
        extract(&second),
        "the same seed must reproduce the same shrunk counterexample"
    );
}

fn seed_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/property_seed")
}

/// The same properties beside a module of plain cases.
fn mixed_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/property_seed_mixed")
}

/// `bynkc test <fixture> --format json --seed <seed> [--case <name>]` and its
/// parsed document.
fn json_run(out_dir: &str, seed: &str, case: Option<&str>) -> serde_json::Value {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynkc"));
    cmd.arg("test")
        .arg(seed_fixture())
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", seed]);
    if let Some(case) = case {
        cmd.args(["--case", case]);
    }
    let out = cmd.output().expect("run bynkc test");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "a JSON document: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn case_named<'a>(doc: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    doc["suites"]
        .as_array()
        .expect("suites")
        .iter()
        .flat_map(|s| s["cases"].as_array().expect("cases"))
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("case {name:?} is in the document: {doc}"))
}

/// Track C (#2266): a failed property reports its seed and counterexample as
/// data; a passing one reports neither; and `--case` + `--seed <root>` reproduces
/// the failure with the same case count, because the property's stream depends
/// on the root seed and its ordinal only.
#[test]
fn a_failed_property_reports_its_seed_and_counterexample_as_data() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    let whole = json_run("out-data-a", "0x5f3a", None);
    let failed = case_named(&whole, "never seventy three");
    assert_eq!(failed["outcome"], "fail");
    assert_eq!(failed["kind"], "property");
    assert_eq!(
        failed["seed"], "0x5f3a",
        "the root seed, as `--seed` accepts it"
    );
    assert_eq!(failed["counterexample"], "p = 73");
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .contains("property failed after 74 cases (seed 0x5f3a)"),
        "the message text is unchanged: {failed}"
    );
    let passed = case_named(&whole, "stays at least zero");
    assert_eq!(passed["outcome"], "pass");
    for field in ["kind", "seed", "counterexample"] {
        assert!(
            passed.get(field).is_none(),
            "a passing case has no {field}: {passed}"
        );
    }

    // Reproduce just that case from its reported seed.
    let alone = json_run("out-data-b", "0x5f3a", Some("never seventy three"));
    let again = case_named(&alone, "never seventy three");
    assert_eq!(again["seed"], failed["seed"]);
    assert_eq!(again["counterexample"], failed["counterexample"]);
    assert_eq!(
        again["message"], failed["message"],
        "`--case` + `--seed` reproduces the failure and its case count"
    );
}

/// Track C (#2266): the runner reads `kind`, `seed` and `counterexample` off
/// every module's results, so a project mixing a property module with a module of
/// plain cases must still type-check: only the former's result type has them.
#[test]
fn a_project_mixing_property_and_plain_modules_type_checks() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("out-mixed")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(mixed_fixture())
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynkc test");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "a JSON document: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert!(
        doc.get("error").is_none(),
        "the generated TypeScript type-checks: {doc}"
    );
    let failed = case_named(&doc, "never seventy three");
    assert_eq!(failed["seed"], "0x5f3a");
    assert_eq!(failed["counterexample"], "p = 73");
    assert_eq!(case_named(&doc, "doubles two")["outcome"], "pass");
}

fn invariant_fail_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/behaviour/property_invariant_fail")
}

/// #2272: an invariant violation inside a property is a shrinkable failure, so it
/// reports a seed and a counterexample; a failed `expect` reports its own
/// `path:line:col` rather than the property's file path.
#[test]
fn a_violated_invariant_shrinks_and_a_failed_expect_reports_its_position() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("out-invariant")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(invariant_fail_fixture())
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynkc test");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "a JSON document: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });

    let violated = case_named(&doc, "spend keeps balance nonneg");
    assert_eq!(violated["outcome"], "fail");
    assert_eq!(violated["kind"], "property");
    assert_eq!(violated["seed"], "0x5f3a");
    assert_eq!(violated["counterexample"], "n = 1");
    assert!(
        violated["message"]
            .as_str()
            .unwrap()
            .contains("InvariantViolation: Wallet.nonneg"),
        "the violation is named: {violated}"
    );

    let expect = case_named(&doc, "amounts stay small");
    assert_eq!(expect["outcome"], "fail");
    assert_eq!(expect["counterexample"], "n = 3");
    assert_eq!(
        expect["location"]["path"], "src/tests/wallet.test.bynk",
        "the failing expect's file: {expect}"
    );
    assert_eq!(
        expect["location"]["line"], 10,
        "the failing expect's line: {expect}"
    );
}
