//! #1426 regression: a generative (`for all`/contract-attack) `Int` binding
//! draws its value as a JS `bigint` (`rng.int(...)`), but ordinary Bynk `Int`
//! arithmetic lowers to plain TypeScript `+`/`-`/`*`, which throws `TypeError:
//! Cannot mix BigInt and other types` the instant it meets a `number` literal.
//! `positive_fixtures`/`tsc_verify` only diff golden text and type-check
//! (`__vals: any[]`, so the bug is invisible to `tsc --strict`) — this proves
//! the *runtime* fix: drives the real `bynkc test` CLI end to end (the same
//! shape `property_behaviour.rs`/`contract_behaviour.rs` use) and asserts a
//! clean pass, not merely that the generated text matches a golden file.

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

fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

/// Runs `bynkc test <fixture>` and returns combined stdout+stderr. Each fixture
/// gets its own `out_dir` name — a shared parent would race the emitted `.js` a
/// concurrent test's `tsc`/`node` run reads (same reason `property_behaviour.rs`/
/// `contract_behaviour.rs` isolate their own runs).
fn run(fixture: &str, out_dir: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/positive/{fixture}"));
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(&path)
        .arg("--output")
        .arg(&out_root)
        .output()
        .expect("run bynkc test");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

/// `expect_cases` names every case the fixture's own `run` output must show
/// as having actually executed. Review of #1432, finding 2: `"0 failed"` is
/// a substring match that also matches `"0 passed, 0 failed."` — passes
/// vacuously if nothing ran at all, which is exactly the failure mode for
/// the contract-attack fixture specifically (attack runners are only
/// emitted when `contracts` is on, `tests_emit.rs`'s own `emit_test_module`;
/// `positive_fixtures`/`tsc_verify`'s plain `compile` never sets it, so the
/// golden fixture pins nothing about `emit_contract_attack_function`'s own
/// new `destructure` at all — this behavioural test is the *only* thing
/// that does, and needs to positively confirm the attack case ran, not just
/// that nothing failed).
fn assert_clean_pass(output: &str, fixture: &str, expect_cases: &[&str]) {
    assert!(
        !output.contains("Cannot mix BigInt"),
        "fixture `{fixture}` regressed #1426 — bigint/number arithmetic \
         TypeError reappeared:\n{output}"
    );
    for case in expect_cases {
        assert!(
            output.contains(case),
            "fixture `{fixture}` never ran case `{case}`:\n{output}"
        );
    }
    assert!(
        output.contains("0 failed"),
        "fixture `{fixture}` reported a failure:\n{output}"
    );
}

#[test]
fn scalar_int_for_all_binding_participates_in_arithmetic_without_crashing() {
    // `bynkc test`'s runner detection is Unix-only (it shells out to `which`).
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
    let output = run("1426_property_scalar_int_arithmetic", "prop-int-arith");
    assert_clean_pass(
        &output,
        "1426_property_scalar_int_arithmetic",
        &["scalar int participates in arithmetic"],
    );
}

#[test]
fn contract_attack_int_parameter_participates_in_requires_arithmetic_without_crashing() {
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
    let output = run("1426_contract_attack_int_arithmetic", "attack-int-arith");
    assert_clean_pass(
        &output,
        "1426_contract_attack_int_arithmetic",
        &["bump adds one", "contract bump"],
    );
}
