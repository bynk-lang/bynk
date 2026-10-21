//! #2185: `bynkc test` heads a `system` suite's results with its tier.
//!
//! A suite declared `suite shop.orders as system { … }` runs as built Workers
//! over the JSON wire, and its human results heading is `system · shop.orders:`.
//! It used to read `integration · shop.orders:`, a leftover from the retired
//! standalone `integration` block the wired path grew out of. The `--format
//! json` document keeps its pinned suite `kind` (`"integration"` for a wired
//! suite) and the bare suite `name`. Suites still run, and are listed, in
//! heading order, so beside unit suites a `system` suite now sorts under `s`.
//!
//! This drives the real CLI against fixture 171 (a two-context `system` suite).
//! Like the other toolchain-driving tests it skips on Windows and skips loudly
//! when no TypeScript runner is on `PATH`; `BYNK_REQUIRE_TSC` turns the skip
//! into a failure (CI).

use std::path::PathBuf;
use std::process::Command;

mod require;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The same fallback chain `bynkc test` itself walks.
fn have_runner() -> bool {
    tool_exists("tsx") || (tool_exists("tsc") && tool_exists("node")) || tool_exists("npx")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/171_integration_two_context_service/src")
}

/// Run `bynkc test` on the fixture with `extra` arguments, into its own
/// output tree (the runner writes a sibling `out-js`, so runs must not share
/// a parent).
fn run(out_dir: &str, extra: &[&str]) -> std::process::Output {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(out_dir)
        .join("out");
    Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .args(extra)
        .output()
        .expect("run bynkc test")
}

#[test]
fn a_system_suite_is_headed_system() {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return;
    }
    if !have_runner() {
        if require::is_required(REQUIRE_ENV) {
            panic!("no TypeScript runner (tsx or tsc+node) on PATH, but {REQUIRE_ENV} is set");
        }
        eprintln!("skipping: no TypeScript runner (tsx or tsc+node) on PATH");
        return;
    }

    let out = run("system-suite-heading-rich", &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the suite should pass:\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.lines().any(|l| l == "system · shop.orders:"),
        "a `system` suite is headed by its tier, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("integration ·"),
        "a `system` suite is not headed `integration`, got:\n{stdout}"
    );

    // The JSON document is unchanged: the bare suite name, and its pinned kind.
    let out = run("system-suite-heading-json", &["--format", "json"]);
    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("`--format json` emits a JSON document");
    let suites = doc["suites"].as_array().expect("suites array");
    assert_eq!(suites.len(), 1, "{doc}");
    assert_eq!(suites[0]["name"], "shop.orders", "{doc}");
    assert_eq!(suites[0]["kind"], "integration", "{doc}");
}
