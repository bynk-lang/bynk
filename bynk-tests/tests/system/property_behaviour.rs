//! v0.114 behavioural test for generative `property` tests (testing track slice
//! 2). Snapshots prove the emitted shape; this proves the runtime behaviour: a
//! property that fails over its domain reports a case count, the run's root
//! seed, and a shrunk counterexample with a copy-paste reproduce line — and
//! `--seed <hex>` reproduces the run byte-for-byte.
//!
//! Drives the real `bynk test` CLI against a fixture project, so it exercises
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
/// PATH — the same fallback chain `bynk test` itself walks.
fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/property_fail")
}

/// Run `bynk test <fixture> --seed <seed>` and return combined stdout+stderr.
fn run_with_seed(seed: &str, out_dir: &str) -> String {
    // Emit under the target tmpdir so no build artifacts land in the source tree.
    // Each run gets its own output dir: the runner writes the executed `out-js` tree
    // inside `--output` (#4159), so concurrent runs cannot race each other's `.js`.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .arg("--seed")
        .arg(seed)
        .output()
        .expect("run bynk test");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[test]
fn failing_property_reports_seed_and_shrunk_counterexample() {
    // `bynk test`'s runner detection is Unix-only (it shells out to `which`), so
    // the CLI cannot locate `tsc`/`node`/`tsx` on Windows and never runs the
    // emitted tests. This test drives that CLI end to end, so it is meaningful
    // only where the CLI can run a runner — skip on Windows. (The *emission* is
    // covered platform-independently by the golden fixture `243_property_passes`.)
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
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

/// `bynk test <fixture> --format json --seed <seed> [--case <name>]` and its
/// parsed document.
fn json_run(out_dir: &str, seed: &str, case: Option<&str>) -> serde_json::Value {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"));
    cmd.arg("test")
        .arg(seed_fixture())
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", seed]);
    if let Some(case) = case {
        cmd.args(["--case", case]);
    }
    let out = cmd.output().expect("run bynk test");
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
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
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
            // #4199: a widened draw changes which case first hits the counterexample.
            .contains("property failed after 38 cases (seed 0x5f3a)"),
        "the message text is the pinned one: {failed}"
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
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
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
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(mixed_fixture())
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynk test");
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
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
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
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(invariant_fail_fixture())
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynk test");
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
    // #2272: the shrunk run names the invariant it violated, and where it is declared.
    assert_eq!(violated["violation"]["kind"], "invariant");
    assert_eq!(violated["violation"]["agent"], "Wallet");
    assert_eq!(violated["violation"]["name"], "nonneg");
    assert_eq!(violated["violation"]["location"]["line"], 10);
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

/// #4198: a shrunk counterexample over `Decimal` or `Bytes` prints in Bynk's own
/// form, not `[object Object]` (a `bigint` makes `JSON.stringify` throw) or an
/// index map.
#[test]
fn a_decimal_or_bytes_counterexample_prints_in_bynk_form() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
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
        .join("out-show")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/behaviour/property_show"),
        )
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynk test");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "a JSON document: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(
        case_named(&doc, "a decimal differs from itself")["counterexample"],
        "x = 0d"
    );
    assert_eq!(
        case_named(&doc, "bytes differ from themselves")["counterexample"],
        "b = Bytes.fromHex(\"\")"
    );
    // #4229: a `Duration` and an `Instant` print in Bynk's form, not as bare milliseconds.
    assert_eq!(
        case_named(&doc, "durations differ from themselves")["counterexample"],
        "d = 0.seconds"
    );
    assert_eq!(
        case_named(&doc, "instants differ from themselves")["counterexample"],
        "t = Instant.fromEpochMillis(0)"
    );
}

/// #4199: `for all` draws each plain type across its realistic domain, so a claim that held only
/// in the old narrow domain (an `Int` within 1000, a string of at most eight lowercase
/// characters, a float below 1001, a duration under a day) is found false and shrunk to a small
/// counterexample; a refined `Int` still stays inside its range.
#[test]
fn for_all_draws_across_the_realistic_domain_of_each_plain_type() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynk test` runner detection is Unix-only");
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
        .join("out-domain")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/behaviour/property_domain"),
        )
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynk test");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "a JSON document: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let shown = |name: &str| {
        let c = case_named(&doc, name);
        assert_eq!(c["outcome"], "fail", "{name}: {c}");
        c["counterexample"].as_str().unwrap().to_string()
    };
    // An `Int` beyond 1000 shrinks to the smallest value that breaks the claim.
    assert_eq!(shown("ints stay below 1001"), "n = 1001");
    // A string draws whitespace, punctuation, accented, CJK or astral characters beside a-z.
    let s = shown("strings stay within eight characters");
    assert!(
        s.chars()
            .any(|c| !c.is_ascii_lowercase() && c != '"' && c != ' ' && c != '=' && c != 's'),
        "a string outside lowercase a-z: {s}"
    );
    // A float beyond 1000 shrinks to an ordinary size.
    let x: f64 = shown("floats stay below 1001")
        .trim_start_matches("x = ")
        .parse()
        .expect("a float");
    assert!((1001.0..1_000_000.0).contains(&x), "{x}");
    // A duration beyond a day.
    assert_eq!(shown("durations stay under a day"), "d = 1.day");
    // These claims fail only inside a band no boundary value reaches, so they are found false
    // through the random draw alone. The first number in a counterexample is the value.
    // The counterexample prints in Bynk form: a decimal literal, `<n>.<unit>` for a duration, or
    // `Instant.fromEpochMillis(n)`; read each back as a number (milliseconds for the times).
    let number = |name: &str| -> f64 {
        let text = shown(name);
        let start = text.find(|c: char| c.is_ascii_digit()).expect("a number");
        let digits: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let digits = digits.trim_end_matches('.');
        let n: f64 = digits.parse().expect("a number");
        let unit = &text[start + digits.len()..];
        let unit = unit.trim_start_matches('.');
        let per = [
            ("milliseconds", 1.0),
            ("millisecond", 1.0),
            ("seconds", 1_000.0),
            ("second", 1_000.0),
            ("minutes", 60_000.0),
            ("minute", 60_000.0),
            ("hours", 3_600_000.0),
            ("hour", 3_600_000.0),
            ("days", 86_400_000.0),
            ("day", 86_400_000.0),
        ];
        match per.iter().find(|(u, _)| unit.starts_with(u)) {
            Some((_, ms)) if name.contains("duration") => n * ms,
            _ => n,
        }
    };
    let d = number("decimals stay out of the thousands");
    assert!((1000.0..100_000_000.0).contains(&d), "{d}");
    let hex = shown("byte strings stay short");
    assert!(
        hex.matches("00").count() + hex.matches("ff").count() >= 5 || hex.len() > 24,
        "{hex}"
    );
    let d = number("durations avoid two days to a year");
    assert!((172_800_000.0..31_536_000_000.0).contains(&d), "{d}");
    let t = number("instants avoid a band of epoch milliseconds");
    assert!((200_000_000.0..30_000_000_000.0).contains(&t), "{t}");
    assert!(number("floats stay below a trillion") >= 1e12);
    // A refinement narrows the draw: the claim over `Percent` holds.
    for name in [
        "a refined int stays inside its range",
        "a length-bounded string stays within its limit",
    ] {
        assert_eq!(case_named(&doc, name)["outcome"], "pass", "{name}");
    }
}
