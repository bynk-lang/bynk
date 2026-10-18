//! #854 behavioural test for `bynkc test --coverage`. Drives the real `bynkc
//! test` CLI against fixture projects and asserts the coverage it reports is
//! attributed to `.bynk` source lines — the exact covered/uncovered sets, the
//! `tsc → node`-only guards (DECISION C), and source-only attribution for a
//! project whose integration suite runs the workers scaffold (DECISION D).
//!
//! Coverage needs the `tsc → node` path specifically (it consumes tsc's
//! `.js.map`s), so — unlike the `tsx`-tolerant runner tests — this skips loudly
//! unless `node` and `tsc`/`npx` are present. `BYNK_REQUIRE_TSC=1` turns the skip
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

/// Coverage requires `node` plus a `tsc` (direct or `npx`-provisioned) — the
/// path that produces the `.js.map`s the remap reads. `tsx` alone does not
/// qualify (DECISION C).
fn have_coverage_runner() -> bool {
    tool_exists("node") && (tool_exists("tsc") || tool_exists("npx"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/behaviour/{name}"))
}

/// Run `bynkc test <fixture> --coverage [--format json]` and return combined
/// stdout+stderr. Each run gets its own parent dir: the runner derives the
/// executed `out-js` tree as a *sibling* of `--output`, so a shared parent would
/// race the emitted `.js`/`.js.map`s the remap reads back.
fn run_coverage(name: &str, out_dir: &str, json: bool) -> String {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynkc"));
    cmd.arg("test")
        .arg(fixture(name))
        .arg("--output")
        .arg(&out_root)
        .arg("--coverage");
    if json {
        cmd.arg("--format").arg("json");
    }
    let out = cmd.output().expect("run bynkc test --coverage");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

fn skip_or_fail() -> bool {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return true;
    }
    if !have_coverage_runner() {
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("no `tsc`+`node` coverage runner on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no `tsc`+`node` coverage runner on PATH");
        return true;
    }
    false
}

#[test]
fn partial_coverage_reports_the_exact_uncovered_lines() {
    if skip_or_fail() {
        return;
    }
    // `demo.m` declares `double` (tested) and `triple` (never called). Coverage
    // is attributed to `.bynk` lines: `double`'s two lines run, `triple`'s three
    // (its `fn`, `let a`, `a + n`) do not — despite the module body around its
    // `export function triple` running at load, which a naive remap would
    // miscount as covered (#2146: the runner compiles to ES modules).
    let rich = run_coverage("coverage_partial", "cov-partial-rich", false);
    assert!(
        rich.contains("src/demo/m.bynk"),
        "coverage is keyed by the `.bynk` source, got:\n{rich}"
    );
    assert!(
        rich.contains("(2/5 lines)") && rich.contains("40%"),
        "two of five executable lines covered, got:\n{rich}"
    );
    assert!(
        rich.contains("uncovered: 7-9"),
        "the uncalled `triple`'s lines 7-9 are uncovered, got:\n{rich}"
    );

    // The JSON surface carries an equivalent `coverage` block.
    let json = run_coverage("coverage_partial", "cov-partial-json", true);
    assert!(
        json.contains("\"coverage\""),
        "the JSON document carries a coverage block, got:\n{json}"
    );
    assert!(
        json.contains("\"path\": \"src/demo/m.bynk\"")
            && json.contains("\"covered\": 2")
            && json.contains("\"lines\": 5"),
        "the coverage block pins the file and its counts, got:\n{json}"
    );
    for want in ["7", "8", "9"] {
        assert!(
            json.contains(want),
            "uncovered line {want} is present, got:\n{json}"
        );
    }
    // #2104: both line lists, exactly, so an editor can mark covered lines as
    // well as uncovered ones. `double`'s body (lines 3-4) ran; `triple`'s
    // (7-9) did not.
    let start = json.find('{').expect("a JSON document");
    let doc: serde_json::Value = serde_json::Deserializer::from_str(&json[start..])
        .into_iter::<serde_json::Value>()
        .next()
        .expect("one document")
        .expect("valid JSON");
    let file = &doc["coverage"]["files"][0];
    assert_eq!(file["uncovered"], serde_json::json!([7, 8, 9]), "{json}");
    assert_eq!(file["covered_lines"], serde_json::json!([3, 4]), "{json}");
}

#[test]
fn full_coverage_reports_no_uncovered_lines() {
    if skip_or_fail() {
        return;
    }
    // Every fn in `calc.pure` is exercised, directly or transitively, so the file
    // reports 100% and no `uncovered:` clause at all.
    let rich = run_coverage("coverage_full", "cov-full-rich", false);
    assert!(
        rich.contains("src/calc/pure.bynk") && rich.contains("100%"),
        "the pure commons is fully covered, got:\n{rich}"
    );
    assert!(
        rich.contains("(5/5 lines)"),
        "all five executable lines covered, got:\n{rich}"
    );
    assert!(
        !rich.contains("uncovered:"),
        "a fully-covered file lists no uncovered lines, got:\n{rich}"
    );
}

#[test]
fn integration_suite_attributes_only_to_source() {
    if skip_or_fail() {
        return;
    }
    // The project runs an integration (`system`) suite across the real Worker
    // boundary *and* a plain unit suite over the `shop.util` commons. DECISION D:
    // coverage is attributed to `.bynk` source only — the workers scaffold the
    // integration suite executes is filtered out, and no generated `.ts`/`.js`
    // appears. So the sole measured file is the unit-tested commons.
    let rich = run_coverage("coverage_integration", "cov-integration-rich", false);
    assert!(
        rich.contains("2 passed"),
        "the unit case and the integration case both run, got:\n{rich}"
    );
    assert!(
        rich.contains("src/shop/util.bynk") && rich.contains("uncovered: 7-9"),
        "coverage lands on the unit-tested commons, got:\n{rich}"
    );
    // Source-only: nothing from the workers scaffold, the contexts' generated
    // code, or the test tree is attributed.
    for forbidden in ["workers", "shop/orders.bynk", "shop/payment.bynk", ".ts"] {
        assert!(
            !rich.contains(forbidden),
            "coverage must not attribute to `{forbidden}`, got:\n{rich}"
        );
    }
}

#[test]
fn coverage_rejects_inspect_and_no_run() {
    if skip_or_fail() {
        return;
    }
    // DECISION C: `--coverage` requires the `tsc → node` run; `--inspect` and
    // `--no-run` are rejected with an actionable message rather than silently
    // producing nothing.
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cov-guard/out");

    // `--coverage --inspect` — the two run models are exclusive.
    let inspect = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test"])
        .arg(fixture("coverage_partial"))
        .arg("--output")
        .arg(&out_root)
        .args(["--coverage", "--inspect"])
        .output()
        .expect("run");
    assert!(!inspect.status.success(), "coverage + inspect fails");
    let msg = String::from_utf8_lossy(&inspect.stderr);
    assert!(
        msg.contains("--inspect"),
        "the error names the offending flag, got:\n{msg}"
    );

    // `--coverage --no-run` — there is no run to measure. A distinct early-return
    // with its own message, so exercise it separately.
    let no_run = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test"])
        .arg(fixture("coverage_partial"))
        .arg("--output")
        .arg(&out_root)
        .args(["--coverage", "--no-run"])
        .output()
        .expect("run");
    assert!(!no_run.status.success(), "coverage + no-run fails");
    let msg = String::from_utf8_lossy(&no_run.stderr);
    assert!(
        msg.contains("--no-run"),
        "the error names the offending flag, got:\n{msg}"
    );
}
