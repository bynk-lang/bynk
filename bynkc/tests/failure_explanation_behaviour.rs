//! #2272 (Slice F spike) baseline: how the failures Slice F sets out to explain are
//! reported today. Drives the real CLI against the `failure_explanation` fixture and
//! pins each finding of the spike, so the change that explains a failure shows up as
//! a changed assertion here rather than as an unnoticed difference.
//!
//! Each block says what it pins and what Slice F would change. A finding a change has
//! fixed is asserted in its fixed form, so a regression back to the bare string fails.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn can_run() -> bool {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return false;
    }
    if !(tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")) {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return false;
    }
    true
}

/// The run's document: the fixture at a fixed seed, so the shrunk values are stable.
/// Every test reads the same document, so the CLI runs once. Running it per test, in
/// parallel threads, would share one `--output`, and the runner derives the executed
/// tree as a sibling of that directory, so the emitted `.js` of one run would race
/// another's.
fn run() -> &'static serde_json::Value {
    static DOC: OnceLock<serde_json::Value> = OnceLock::new();
    DOC.get_or_init(run_once)
}

fn run_once() -> serde_json::Value {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/behaviour/failure_explanation");
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("failure-explanation")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(project)
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json", "--seed", "0x5f3a"])
        .output()
        .expect("run bynkc test");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON document: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn case<'a>(doc: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    doc["suites"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["cases"].as_array().into_iter().flatten())
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no case named {name:?} in {doc}"))
}

fn message(c: &serde_json::Value) -> &str {
    c["message"].as_str().unwrap_or_default()
}

#[test]
fn a_violation_in_a_case_is_a_bare_string_with_no_location() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 1: the message names the agent and the declaration, and nothing else:
    // not the call, not the state, and no `location` to navigate to.
    let invariant = case(doc, "an overdraw breaks the invariant");
    assert_eq!(invariant["outcome"], "fail");
    assert_eq!(
        message(invariant),
        "Error: InvariantViolation: Wallet.nonneg"
    );
    assert!(invariant.get("location").is_none(), "{invariant}");
    // #2272: the declaration itself is now reported, with its kind and position.
    assert_eq!(invariant["violation"]["kind"], "invariant");
    assert_eq!(invariant["violation"]["agent"], "Wallet");
    assert_eq!(invariant["violation"]["name"], "nonneg");
    assert_eq!(
        invariant["violation"]["location"]["path"],
        "src/demo/wallet.bynk"
    );
    assert_eq!(invariant["violation"]["location"]["line"], 9);
    assert_eq!(invariant["violation"]["location"]["col"], 13);

    // Finding 2: a transition is reported in the same words, as `InvariantViolation`.
    // Nothing in the message says it was a transition that fired.
    let transition = case(doc, "a take breaks the transition");
    assert_eq!(transition["outcome"], "fail");
    assert_eq!(
        message(transition),
        "Error: InvariantViolation: Ledger.never_decreases"
    );
    assert!(transition.get("location").is_none(), "{transition}");
    assert_eq!(transition["violation"]["kind"], "transition");
    assert_eq!(transition["violation"]["agent"], "Ledger");
    assert_eq!(transition["violation"]["name"], "never_decreases");
    assert_eq!(transition["violation"]["location"]["line"], 18);
    assert_eq!(transition["violation"]["location"]["col"], 14);
}

#[test]
fn an_invariant_and_a_transition_with_one_name_are_reported_identically() {
    if !can_run() {
        return;
    }
    let doc = run();

    // The checker allows an invariant and a transition to share a name, so `Agent.name`
    // does not say which declaration fired. A consumer that resolves the declaration by
    // that name alone can pick the wrong one; the kind has to come from the runner.
    let by_transition = case(doc, "a drop breaks the transition named same");
    let by_invariant = case(doc, "a drop breaks the invariant named same");
    assert_eq!(by_transition["outcome"], "fail");
    assert_eq!(by_invariant["outcome"], "fail");
    assert_eq!(
        message(by_transition),
        "Error: InvariantViolation: Meter.same"
    );
    assert_eq!(
        message(by_invariant),
        "Error: InvariantViolation: Meter.same"
    );
    // #2272: the runner says which declaration fired, so the shared name no longer
    // decides it: the same name, two kinds, two positions.
    assert_eq!(by_transition["violation"]["kind"], "transition");
    assert_eq!(by_transition["violation"]["location"]["line"], 28);
    assert_eq!(by_invariant["violation"]["kind"], "invariant");
    assert_eq!(by_invariant["violation"]["location"]["line"], 27);
}

#[test]
fn a_property_that_violates_an_invariant_is_shrunk() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 3, fixed (#2272): `__bynkIsFailure` counts a violation as a shrinkable
    // failure, so the violating property shrinks to `n = 1` and reports a counterexample
    // beside its seed. The violation itself still carries no location of its own (the
    // declaration's position is the next slice), so there is no `location` here.
    let violating = case(doc, "spending any amount breaks the invariant");
    assert_eq!(violating["outcome"], "fail");
    assert_eq!(violating["kind"], "property");
    assert_eq!(violating["seed"], "0x5f3a");
    assert_eq!(violating["counterexample"], "n = 1");
    assert!(
        message(violating).contains("InvariantViolation: Wallet.nonneg"),
        "{violating}"
    );
    assert!(violating.get("location").is_none(), "{violating}");
    // The shrunk run names the declaration it violated.
    assert_eq!(violating["violation"]["kind"], "invariant");
    assert_eq!(violating["violation"]["name"], "nonneg");
}

#[test]
fn a_failed_property_or_history_reports_the_position_of_its_expect() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 4, fixed (#2272): the location is taken from the failing `expect` in the
    // shrunk re-run, so the report links to the line that failed, not the property's file.
    let property = case(doc, "every amount exceeds five (false)");
    assert_eq!(property["counterexample"], "n = 1");
    assert!(message(property).contains("expect n > 5"), "{property}");
    assert_eq!(property["location"]["path"], "src/tests/wallet.test.bynk");
    assert_eq!(property["location"]["line"], 32, "{property}");

    let history = case(doc, "every call is accepted (false)");
    assert_eq!(history["kind"], "history");
    assert!(
        message(history).contains("expect run.all((s) => s.accepted)"),
        "{history}"
    );
    assert_eq!(history["location"]["path"], "src/tests/wallet.test.bynk");
    assert_eq!(history["location"]["line"], 46, "{history}");
}

#[test]
fn a_refused_step_in_a_history_is_data_and_the_report_names_the_declaration_that_refused_it() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 5: a driven history does not fail or go unshrunk when a step violates
    // an invariant. The driver catches the violation and records the step as
    // `accepted: false` (ADR 0155 D4), so a history that does not look at `accepted`
    // passes although the generated runs overdraw the wallet.
    let tolerant = case(doc, "every call is a top-up or a spend");
    assert_eq!(tolerant["outcome"], "pass", "{tolerant}");
    assert!(tolerant.get("refusals").is_none(), "{tolerant}");

    // One that does look reports a shrunk sequence, and now the declaration that
    // refused each step in it. The refusal is not what failed the case, so there is
    // no `violation`.
    let strict = case(doc, "every call is accepted (false)");
    assert_eq!(strict["outcome"], "fail");
    assert_eq!(strict["counterexample"], "[Spend(1)]");
    assert!(strict.get("violation").is_none(), "{strict}");
    let refusals = strict["refusals"].as_array().expect("refusals");
    assert_eq!(refusals.len(), 1, "{strict}");
    assert_eq!(refusals[0]["index"], 0);
    assert_eq!(refusals[0]["call"], "Spend");
    assert_eq!(refusals[0]["kind"], "invariant");
    assert_eq!(refusals[0]["agent"], "Wallet");
    assert_eq!(refusals[0]["name"], "nonneg");
    assert_eq!(refusals[0]["location"]["path"], "src/demo/wallet.bynk");
    assert_eq!(refusals[0]["location"]["line"], 9);
}
