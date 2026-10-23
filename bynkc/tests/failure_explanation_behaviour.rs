//! #2272 (Slice F spike) baseline: how the failures Slice F sets out to explain are
//! reported today. Drives the real CLI against the `failure_explanation` fixture and
//! pins each finding of the spike, so the change that explains a failure shows up as
//! a changed assertion here rather than as an unnoticed difference.
//!
//! Findings 3 and 4 are fixed (a violation shrinks; a property or history links to
//! its failing `expect`), and findings 1 and 2 are fixed in part: a refused case
//! names the declaration and its kind. What is still the behaviour of today: the
//! message is the bare string, the declaration has no `location`, and a refusal
//! inside a driven history is not attributed to a step (finding 5). Each block says
//! what it pins and what would change it.
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
    DOC.get_or_init(|| run_once("failure_explanation"))
}

/// As [`run`], for the fixture with a `system` suite, in an output directory of its
/// own so that the two runs never share one.
fn run_system() -> &'static serde_json::Value {
    static DOC: OnceLock<serde_json::Value> = OnceLock::new();
    DOC.get_or_init(|| run_once("failure_explanation_system"))
}

fn run_once(fixture: &str) -> serde_json::Value {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/behaviour")
        .join(fixture);
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(fixture.replace('_', "-"))
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

/// The `violation` object of a case, or null.
fn violation(c: &serde_json::Value) -> &serde_json::Value {
    &c["violation"]
}

#[test]
fn a_violation_in_a_case_names_the_declaration_but_has_no_location() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Findings 1 and 2, fixed in part: the case now says which declaration refused
    // it, and whether that was an invariant or a transition, as data. What is not
    // fixed: the message is still the bare string (no call, no state), and there is
    // no `location` to navigate to, because the declaration's position is not
    // reported yet.
    let invariant = case(doc, "an overdraw breaks the invariant");
    assert_eq!(invariant["outcome"], "fail");
    assert_eq!(
        message(invariant),
        "Error: InvariantViolation: Wallet.nonneg"
    );
    assert_eq!(
        *violation(invariant),
        serde_json::json!({"kind": "invariant", "agent": "Wallet", "name": "nonneg"})
    );
    assert!(invariant.get("location").is_none(), "{invariant}");

    let transition = case(doc, "a take breaks the transition");
    assert_eq!(transition["outcome"], "fail");
    assert_eq!(
        message(transition),
        "Error: InvariantViolation: Ledger.never_decreases"
    );
    assert_eq!(
        *violation(transition),
        serde_json::json!({"kind": "transition", "agent": "Ledger", "name": "never_decreases"})
    );
    assert!(transition.get("location").is_none(), "{transition}");
}

#[test]
fn an_invariant_and_a_transition_with_one_name_are_told_apart_by_their_kind() {
    if !can_run() {
        return;
    }
    let doc = run();

    // The checker allows an invariant and a transition to share a name, and their
    // messages are the same words, so `Agent.name` does not say which declaration
    // fired. The runner does: the kind comes from the declaration that threw.
    let by_transition = case(doc, "a drop breaks the transition named same");
    let by_invariant = case(doc, "a drop breaks the invariant named same");
    assert_eq!(by_transition["outcome"], "fail");
    assert_eq!(by_invariant["outcome"], "fail");
    assert_eq!(
        message(by_transition),
        message(by_invariant),
        "the text alone cannot tell them apart"
    );
    assert_eq!(violation(by_transition)["kind"], "transition");
    assert_eq!(violation(by_invariant)["kind"], "invariant");
    assert_eq!(violation(by_transition)["name"], "same");
    assert_eq!(violation(by_invariant)["name"], "same");
}

#[test]
fn a_failure_that_no_declaration_refused_carries_no_violation() {
    if !can_run() {
        return;
    }
    let doc = run();

    // An `expect` that failed, in a property or a history, is not a refusal.
    for name in [
        "every amount exceeds five (false)",
        "every call is accepted (false)",
    ] {
        let failed = case(doc, name);
        assert_eq!(failed["outcome"], "fail");
        assert!(failed.get("violation").is_none(), "{failed}");
    }
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
    // ...and names the declaration it provoked, as a case does.
    assert_eq!(
        *violation(violating),
        serde_json::json!({"kind": "invariant", "agent": "Wallet", "name": "nonneg"})
    );
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
fn a_refused_step_in_a_history_is_data_and_the_report_does_not_say_which_declaration_refused_it() {
    if !can_run() {
        return;
    }
    let doc = run();

    // Finding 5, corrected: a driven history does not fail or go unshrunk when a step
    // violates an invariant. The driver catches the violation and records the step as
    // `accepted: false` (ADR 0155 D4), so a history that does not look at `accepted`
    // passes although the generated runs overdraw the wallet.
    let tolerant = case(doc, "every call is a top-up or a spend");
    assert_eq!(tolerant["outcome"], "pass", "{tolerant}");

    // One that does look reports a shrunk sequence, which names the refused call but
    // not the declaration that refused it: that is dropped where the violation is caught.
    let strict = case(doc, "every call is accepted (false)");
    assert_eq!(strict["outcome"], "fail");
    assert_eq!(strict["counterexample"], "[Spend(1)]");
    assert!(!message(strict).contains("nonneg"), "{strict}");
}

#[test]
fn a_refusal_in_a_system_suite_is_not_attributed_to_a_declaration() {
    if !can_run() {
        return;
    }
    let doc = run_system();

    // The agent of a `system` suite runs behind the context's services, so its
    // refusal becomes a fault at the call boundary and the case never holds the fault
    // that carries the refused declaration. Called as a service, the case sees a
    // `BoundaryError: CalleeFaulted`; over HTTP it sees a response that is not `Ok`.
    // Neither carries a `violation`, so its absence on a `system` case does not mean
    // that no declaration refused it. A change that carries the declaration across
    // the boundary changes these assertions.
    let service = case(doc, "a service call overdraws the wallet");
    assert_eq!(service["outcome"], "fail");
    assert_eq!(service["tier"], "system");
    assert_eq!(message(service), "Error: BoundaryError: CalleeFaulted");
    assert!(service.get("violation").is_none(), "{service}");

    let http = case(doc, "an http call overdraws the wallet");
    assert_eq!(http["outcome"], "fail");
    assert_eq!(http["tier"], "system");
    assert!(message(http).contains("expect r is Ok(_)"), "{http}");
    assert!(http.get("violation").is_none(), "{http}");
}
