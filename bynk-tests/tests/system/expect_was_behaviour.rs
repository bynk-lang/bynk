//! #3980: a failed `expect <subject> is …`, and an `expect false` that is a `match` arm, report
//! the value they tested: as `Json.encode` writes it, as a type name for a subject with no codec,
//! or as `<redacted>`. The whole value is the case's `actual` in `--format json`.

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

fn case(doc: &serde_json::Value, name: &str) -> serde_json::Value {
    doc["suites"]
        .as_array()
        .expect("suites")
        .iter()
        .flat_map(|s| s["cases"].as_array().expect("cases"))
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("case {name:?} is in the document: {doc}"))
        .clone()
}

#[test]
fn a_failed_is_and_a_match_arm_report_the_value_tested() {
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
        .join("expect-was-out")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/expect_was"))
        .arg("--output")
        .arg(&out_root)
        .args(["--format", "json"])
        .output()
        .expect("run bynk test");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "a JSON document: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let message = |name: &str| case(&doc, name)["message"].as_str().unwrap().to_string();

    // A sum: its tagged runtime form, and the whole value as `actual`.
    let sum = case(&doc, "a sum subject");
    assert!(
        message("a sum subject")
            .contains("expect r is Ok(_)\n  r was {\"tag\":\"Err\",\"error\":\"not a number: x\"}"),
        "{sum}"
    );
    assert_eq!(
        sum["actual"],
        "{\"tag\":\"Err\",\"error\":\"not a number: x\"}"
    );

    // `expect false` in a `match` arm: the arm and the scrutinee.
    assert!(
        message("a match arm")
            .contains("expect false (in the `_` arm of `match g`)\n  g was {\"tag\":\"Missing\"}"),
        "{}",
        message("a match arm")
    );

    // An `HttpResult`: its tagged value.
    assert!(
        message("an http result").contains("r was {\"tag\":\"Ok\",\"value\":\"hi\"}"),
        "{}",
        message("an http result")
    );

    // No codec: the type, and no `actual`.
    assert!(
        message("a non-codable subject").contains("s was an Option[Stream[Int]]"),
        "{}",
        message("a non-codable subject")
    );
    assert!(case(&doc, "a non-codable subject").get("actual").is_none());

    // A redacted type: neither shown nor in `actual`.
    assert!(
        message("a redacted subject").contains("c was <redacted>"),
        "{}",
        message("a redacted subject")
    );
    assert!(case(&doc, "a redacted subject").get("actual").is_none());

    // A long value is cut at 2,000 characters in the message; `actual` keeps all of it.
    let long = case(&doc, "a long subject");
    let text = long["message"].as_str().unwrap();
    assert!(
        text.contains("… (")
            && text.contains("more characters; `--format json` has the whole value)"),
        "{text}"
    );
    assert!(text.len() < 2400, "{} characters", text.len());
    assert_eq!(
        long["actual"].as_str().unwrap().len(),
        r#"{"tag":"Some","value":""#.len() + 2100 + 2
    );
}
