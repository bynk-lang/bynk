//! #2272 (Slice F spike) baseline: how the failures Slice F sets out to explain are
//! reported today. Drives the real CLI against the `failure_explanation` fixture and
//! pins each finding of the spike, so the change that explains a failure shows up as
//! a changed assertion here rather than as an unnoticed difference.
//!
//! Each block says what it pins and what Slice F would change. These are the
//! current behaviour, not the wanted behaviour.
//!
//! Like the other toolchain-driving tests it skips loudly when no TypeScript
//! runner is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).

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
fn run() -> serde_json::Value {
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
    let invariant = case(&doc, "an overdraw breaks the invariant");
    assert_eq!(invariant["outcome"], "fail");
    assert_eq!(
        message(invariant),
        "Error: InvariantViolation: Wallet.nonneg"
    );
    assert!(invariant.get("location").is_none(), "{invariant}");

    // Finding 2: a transition is reported in the same words, as `InvariantViolation`.
    // Nothing in the message says it was a transition that fired.
    let transition = case(&doc, "a take breaks the transition");
    assert_eq!(transition["outcome"], "fail");
    assert_eq!(
        message(transition),
        "Error: InvariantViolation: Ledger.never_decreases"
    );
    assert!(transition.get("location").is_none(), "{transition}");
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
    let by_transition = case(&doc, "a drop breaks the transition named same");
    let by_invariant = case(&doc, "a drop breaks the invariant named same");
    assert_eq!(
        message(by_transition),
        "Error: InvariantViolation: Meter.same"
    );
    assert_eq!(
        message(by_invariant),
        "Error: InvariantViolation: Meter.same"
    );
}

#[test]
fn a_property_that_violates_an_invariant_is_not_shrunk() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 3: it reports `kind` and the run's `seed` (Slice C), but `__bynkIsFailure`
    // does not count a violation as a failure to shrink, so there is no shrunk
    // `counterexample` and the message is the bare string.
    let violating = case(&doc, "spending any amount breaks the invariant");
    assert_eq!(violating["outcome"], "fail");
    assert_eq!(violating["kind"], "property");
    assert_eq!(violating["seed"], "0x5f3a");
    assert!(violating.get("counterexample").is_none(), "{violating}");
    assert_eq!(
        message(violating),
        "Error: InvariantViolation: Wallet.nonneg"
    );
}

#[test]
fn a_failed_property_or_history_loses_the_position_of_its_expect() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 4: the shrunk counterexample is reported, and the failing `expect` is
    // named in the message text, but the event carries the file path as its location
    // and the driver drops anything that is not `path:line:col`: no `location`.
    let property = case(&doc, "every amount exceeds five (false)");
    assert_eq!(property["counterexample"], "n = 1");
    assert!(message(property).contains("expect n > 5"), "{property}");
    assert!(property.get("location").is_none(), "{property}");

    let history = case(&doc, "every call is accepted (false)");
    assert_eq!(history["kind"], "history");
    assert!(
        message(history).contains("expect run.all((s) => s.accepted)"),
        "{history}"
    );
    assert!(history.get("location").is_none(), "{history}");
}

#[test]
fn a_refused_step_in_a_history_is_data_and_the_report_does_not_say_which_declaration_refused_it() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 5, corrected: a driven history does not fail or go unshrunk when a step
    // violates an invariant. The driver catches the violation and records the step as
    // `accepted: false` (ADR 0155 D4), so a history that does not look at `accepted`
    // passes although the generated runs overdraw the wallet.
    let tolerant = case(&doc, "every call is a top-up or a spend");
    assert_eq!(tolerant["outcome"], "pass", "{tolerant}");

    // One that does look reports a shrunk sequence, which names the refused call but
    // not the declaration that refused it: that is dropped where the violation is caught.
    let strict = case(&doc, "every call is accepted (false)");
    assert_eq!(strict["outcome"], "fail");
    assert_eq!(strict["counterexample"], "[Spend(1)]");
    assert!(!message(strict).contains("nonneg"), "{strict}");
}
