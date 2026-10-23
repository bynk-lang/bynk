//! v0.59: the `bynkc test --format json` surface.
//!
//! Two toolchain-free layers (proposal v0.59 "Goldening strategy"):
//!  1. **The pinned document** — built from a synthetic [`TestRun`] model and
//!     compared against a committed golden (the `bynk doctor` precedent), for
//!     the normal / compile / runtime shapes.
//!  2. **The NDJSON → document parser** — fed fixture event streams (including a
//!     truncated/crashed one), asserting the folded result.
//!
//! The true end-to-end (node actually emitting NDJSON) is exercised by the
//! toolchain-gated suites, not here, so this stays deterministic on CI.
//!
//! Document goldens are blessed with `BYNK_BLESS=1 cargo test -p bynkc --test it test_json::`.

use std::path::Path;
use std::process::Command;

use bynk_driver::test_json::{Case, Location, Suite, TestRun, parse_ndjson};

fn bless_or_assert(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("BYNK_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; regenerate with BYNK_BLESS=1 cargo test -p bynkc --test it test_json::",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "golden {name} drifted; re-bless with BYNK_BLESS=1 cargo test -p bynkc --test it test_json::"
    );
}

// Fixture event streams (the runner's internal NDJSON protocol).
const NDJSON_COMPLETE: &str = r#"{"type":"run-begin","suites":1}
{"type":"suite-begin","name":"commerce.payment","kind":"unit","tests":2}
{"type":"case","suite":"commerce.payment","name":"charges the card","outcome":"pass"}
{"type":"case","suite":"commerce.payment","name":"rejects expired card","outcome":"fail","message":"assertion failed at tests/commerce/payment.test.bynk:42:5","location":"tests/commerce/payment.test.bynk:42:5"}
{"type":"suite-end","name":"commerce.payment"}
{"type":"run-end","passed":1,"failed":1}
"#;

// A runner that died mid-suite: a complete prefix, but no `run-end`.
const NDJSON_TRUNCATED: &str = r#"{"type":"run-begin","suites":2}
{"type":"suite-begin","name":"orders","kind":"integration","tests":3}
{"type":"case","suite":"orders","name":"places an order","outcome":"pass"}
"#;

// ---------------------------------------------------------------------------
// Document goldens (synthetic model → JSON)
// ---------------------------------------------------------------------------

#[test]
fn golden_document_normal() {
    let run = TestRun {
        passed: 1,
        failed: 1,
        skipped: 0,
        suites: Some(vec![Suite {
            name: "commerce.payment".to_string(),
            kind: "unit".to_string(),
            cases: vec![
                Case {
                    name: "charges the card".to_string(),
                    outcome: "pass".to_string(),
                    message: None,
                    location: None,
                    retries: None,
                    tier: None,
                    kind: None,
                    seed: None,
                    counterexample: None,
                    violation: None,
                    duration_ms: None,
                },
                Case {
                    name: "rejects expired card".to_string(),
                    outcome: "fail".to_string(),
                    message: Some(
                        "assertion failed at tests/commerce/payment.test.bynk:42:5".to_string(),
                    ),
                    location: Some(Location {
                        path: "tests/commerce/payment.test.bynk".to_string(),
                        line: 42,
                        col: 5,
                    }),
                    retries: None,
                    tier: None,
                    kind: None,
                    seed: None,
                    counterexample: None,
                    violation: None,
                    duration_ms: None,
                },
            ],
        }]),
        error: None,
        coverage: None,
        transport: None,
    };
    bless_or_assert("test-json-normal.json", &run.render());
}

#[test]
fn golden_document_discovered() {
    // The `--no-run --format json` document: suites/cases listed without running.
    // Every case is `outcome: "discovered"`, carrying its declaration location;
    // `passed`/`failed` are 0 and there is no `error`. A unit and an integration
    // suite pin both kinds.
    let case = |name: &str, line: u32| Case {
        name: name.to_string(),
        outcome: "discovered".to_string(),
        message: None,
        location: Some(Location {
            path: "tests/commerce/payment.test.bynk".to_string(),
            line,
            col: 8,
        }),
        retries: None,
        tier: None,
        kind: None,
        seed: None,
        counterexample: None,
        violation: None,
        duration_ms: None,
    };
    let run = TestRun::discovered(vec![
        Suite {
            name: "commerce.payment".to_string(),
            kind: "unit".to_string(),
            cases: vec![case("charges the card", 2), case("rejects expired card", 6)],
        },
        Suite {
            name: "checkout".to_string(),
            kind: "integration".to_string(),
            cases: vec![Case {
                name: "places an order".to_string(),
                outcome: "discovered".to_string(),
                message: None,
                location: Some(Location {
                    path: "tests/checkout.test.bynk".to_string(),
                    line: 3,
                    col: 20,
                }),
                retries: None,
                tier: None,
                kind: None,
                seed: None,
                counterexample: None,
                violation: None,
                duration_ms: None,
            }],
        },
    ]);
    bless_or_assert("test-json-discovered.json", &run.render());
}

#[test]
fn golden_document_compile() {
    let run = TestRun::compile_error(vec![
        "src/commerce/payment.bynk:3:5: error[bynk.types.mismatch]: expected `Money`, found `Int`"
            .to_string(),
    ]);
    bless_or_assert("test-json-compile.json", &run.render());
}

#[test]
fn golden_document_runtime() {
    // A crashed run: the observed prefix is kept, plus a `runtime` error.
    let doc = parse_ndjson(NDJSON_TRUNCATED).into_document("RangeError: out of memory\n");
    bless_or_assert("test-json-runtime.json", &doc.render());
}

// ---------------------------------------------------------------------------
// End-to-end discovery (real binary, no toolchain — `--no-run` runs no `tsc`/node)
// ---------------------------------------------------------------------------

/// `bynkc test --no-run --format json` against a real fixture: the compile
/// retains the suite/case manifest and renders it without running. Toolchain-free
/// (no `tsc`/`node`) and side-effect-free (`--no-run` writes no `out/`), so it is
/// safe to point at a committed fixture.
#[test]
fn discovery_lists_cases_without_running() {
    // 106: one `test commerce.payment` with three cases. Pointed at `src/`
    // (single-tree), mirroring the other fixture-driven CLI tests.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/106_context_test_multiple_cases/src");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test"])
        .arg(&fixture)
        .args(["--no-run", "--format", "json"])
        .output()
        .expect("run bynkc test --no-run");
    assert!(
        out.status.success(),
        "discovery should exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("discovery emits a JSON document");
    assert_eq!(doc["passed"], 0);
    assert_eq!(doc["failed"], 0);
    assert!(doc.get("error").is_none(), "discovery is not an error");

    let suites = doc["suites"].as_array().expect("suites array");
    assert_eq!(suites.len(), 1);
    assert_eq!(suites[0]["name"], "commerce.payment");
    assert_eq!(suites[0]["kind"], "unit");

    let cases = suites[0]["cases"].as_array().expect("cases array");
    let names: Vec<&str> = cases.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["case one", "case two", "case three"]);
    for c in cases {
        assert_eq!(
            c["outcome"], "discovered",
            "every case is discovered, not run"
        );
        assert_eq!(c["tier"], "unit", "a case with no `as` clause is unit");
        let loc = &c["location"];
        assert_eq!(loc["path"], "tests/payment.test.bynk");
        assert!(
            loc["line"].as_u64().unwrap() >= 1,
            "carries a 1-indexed line"
        );
        assert!(loc["col"].as_u64().unwrap() >= 1, "carries a 1-indexed col");
    }

    // `--no-run` writes nothing: no `out/` was created beside the sources.
    assert!(
        !fixture.join("out").exists(),
        "discovery must not emit TypeScript"
    );
}

/// Discovery covers `as system` (wired) suites too: kind `"integration"` and the
/// suite's target name (the `system · ` prefix the runner uses is internal).
#[test]
fn discovery_lists_integration_suites() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/171_integration_two_context_service/src");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test"])
        .arg(&fixture)
        .args(["--no-run", "--format", "json"])
        .output()
        .expect("run bynkc test --no-run");
    assert!(
        out.status.success(),
        "discovery should exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("discovery emits a JSON document");
    let suites = doc["suites"].as_array().expect("suites array");
    let integration = suites
        .iter()
        .find(|s| s["kind"] == "integration")
        .expect("the integration suite is discovered");
    assert_eq!(
        integration["name"], "shop.orders",
        "the suite's target name, unprefixed"
    );
    let cases = integration["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    assert!(cases.iter().all(|c| c["outcome"] == "discovered"));
    assert!(
        cases.iter().all(|c| c["tier"] == "system"),
        "the suite is `as system`, so each case is"
    );
}

/// A case's tier is its own, not its suite's `kind`: a case that opts up with
/// `as integration` inside a unit suite reports `integration` while its
/// neighbours report `unit`.
#[test]
fn discovery_reports_each_cases_own_tier() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/291_platform_test_doubles");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test"])
        .arg(&fixture)
        .args(["--no-run", "--format", "json"])
        .output()
        .expect("run bynkc test --no-run");
    assert!(
        out.status.success(),
        "discovery should exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("a JSON document");
    let tier_of = |name: &str| -> String {
        doc["suites"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["cases"].as_array().unwrap())
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("case {name:?} was discovered"))["tier"]
            .as_str()
            .expect("a tier")
            .to_string()
    };
    assert_eq!(tier_of("the clock reads the epoch"), "unit");
    assert_eq!(
        tier_of("the doubles hold at the integration tier"),
        "integration"
    );
}

/// Discovery marks a generative case, so an editor knows which cases a pinned
/// seed's stream depends on without running anything: a `property` is
/// `"property"`, and a plain `case` has no `kind`.
#[test]
fn discovery_marks_generative_cases() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/property_seed");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test"])
        .arg(&fixture)
        .args(["--no-run", "--format", "json"])
        .output()
        .expect("run bynkc test --no-run");
    assert!(
        out.status.success(),
        "discovery should exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("a JSON document");
    let cases = doc["suites"][0]["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 3);
    for c in cases {
        assert_eq!(c["kind"], "property", "{c}");
        assert!(
            c.get("seed").is_none() && c.get("counterexample").is_none(),
            "{c}"
        );
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[test]
fn parser_folds_a_complete_run() {
    let run = parse_ndjson(NDJSON_COMPLETE);
    assert!(run.complete, "a stream with run-end is complete");
    assert_eq!(run.passed, 1);
    assert_eq!(run.failed, 1);
    assert_eq!(run.suites.len(), 1);
    let suite = &run.suites[0];
    assert_eq!(suite.name, "commerce.payment");
    assert_eq!(suite.kind, "unit");
    assert_eq!(suite.cases.len(), 2);
    let fail = &suite.cases[1];
    assert_eq!(fail.outcome, "fail");
    assert_eq!(
        fail.location,
        Some(Location {
            path: "tests/commerce/payment.test.bynk".to_string(),
            line: 42,
            col: 5,
        })
    );
}

#[test]
fn parser_truncated_stream_is_incomplete_and_becomes_runtime_error() {
    let run = parse_ndjson(NDJSON_TRUNCATED);
    assert!(!run.complete, "no run-end ⇒ incomplete");
    assert_eq!(run.passed, 1);
    assert_eq!(run.suites.len(), 1, "the prefix suite is kept");

    let doc = run.into_document("boom\n");
    let err = doc.error.expect("a crashed run carries a runtime error");
    assert_eq!(err.kind, "runtime");
    assert_eq!(err.stderr.as_deref(), Some("boom"));
    assert!(doc.suites.is_some(), "the observed prefix is preserved");
}

#[test]
fn parser_complete_run_has_no_error() {
    let doc = parse_ndjson(NDJSON_COMPLETE).into_document("");
    assert!(doc.error.is_none(), "a complete run is not an error");
    assert_eq!(doc.failed, 1, "failing assertions are not a runtime error");
}

#[test]
fn parser_skips_unparseable_and_unknown_lines() {
    let stream = "not json at all\n{\"type\":\"run-begin\",\"suites\":1}\n{\"type\":\"mystery\"}\n{\"type\":\"run-end\",\"passed\":0,\"failed\":0}\n";
    let run = parse_ndjson(stream);
    assert!(run.complete);
    assert_eq!(run.suites.len(), 0);
}

#[test]
fn parser_unknown_location_yields_no_structured_location() {
    // A non-assertion throw carries `location: "unknown"`, which is not a
    // path:line:col — the case keeps its message but offers no click-through.
    let stream = "{\"type\":\"suite-begin\",\"name\":\"s\",\"kind\":\"unit\",\"tests\":1}\n{\"type\":\"case\",\"suite\":\"s\",\"name\":\"boom\",\"outcome\":\"fail\",\"message\":\"TypeError: x\",\"location\":\"unknown\"}\n{\"type\":\"run-end\",\"passed\":0,\"failed\":1}\n";
    let run = parse_ndjson(stream);
    let case = &run.suites[0].cases[0];
    assert_eq!(case.message.as_deref(), Some("TypeError: x"));
    assert_eq!(case.location, None);
}

#[test]
fn golden_document_with_tiers() {
    // The optional trailing `tier` (ADR 0153's effective tier, attached by the
    // driver): present on a case that has one, absent on one that does not, and
    // always the last key of the case, so a document without it is unchanged.
    let run = TestRun::discovered(vec![Suite {
        name: "orders".to_string(),
        kind: "unit".to_string(),
        cases: vec![
            Case {
                name: "totals".to_string(),
                outcome: "discovered".to_string(),
                message: None,
                location: None,
                retries: None,
                tier: Some("unit".to_string()),
                kind: None,
                seed: None,
                counterexample: None,
                violation: None,
                duration_ms: None,
            },
            Case {
                name: "survives the wire".to_string(),
                outcome: "discovered".to_string(),
                message: None,
                location: None,
                retries: None,
                tier: Some("system".to_string()),
                kind: None,
                seed: None,
                counterexample: None,
                violation: None,
                duration_ms: None,
            },
            Case {
                name: "a property".to_string(),
                outcome: "discovered".to_string(),
                message: None,
                location: None,
                retries: None,
                tier: None,
                kind: None,
                seed: None,
                counterexample: None,
                violation: None,
                duration_ms: None,
            },
        ],
    }]);
    bless_or_assert("test-json-tiers.json", &run.render());
}

#[test]
fn golden_document_failed_property() {
    // A failed generative case carries `kind`, `seed` and (when shrunk)
    // `counterexample`, after `tier` and in that order; a passing case, and a
    // property that threw something other than an assertion, omit what they lack.
    let property = |name: &str, outcome: &str, shrunk: Option<&str>| Case {
        name: name.to_string(),
        outcome: outcome.to_string(),
        message: (outcome == "fail")
            .then(|| "property failed after 74 cases (seed 0x5f3a)".to_string()),
        location: None,
        retries: None,
        tier: None,
        kind: (outcome == "fail").then(|| "property".to_string()),
        seed: (outcome == "fail").then(|| "0x5f3a".to_string()),
        counterexample: shrunk.map(str::to_string),
        violation: None,
        duration_ms: None,
    };
    let run = TestRun {
        passed: 1,
        failed: 2,
        skipped: 0,
        suites: Some(vec![Suite {
            name: "m".to_string(),
            kind: "unit".to_string(),
            cases: vec![
                property("never seventy three", "fail", Some("p = 73")),
                property("throws", "fail", None),
                property("holds", "pass", None),
            ],
        }]),
        error: None,
        coverage: None,
        transport: None,
    };
    bless_or_assert("test-json-failed-property.json", &run.render());
}

#[test]
fn parser_reads_the_generative_fields_of_a_case_event() {
    let run = parse_ndjson(concat!(
        r#"{"type":"suite-begin","name":"m","kind":"unit"}"#,
        "\n",
        r#"{"type":"case","suite":"m","name":"p","outcome":"fail","message":"x","location":"unknown","kind":"history","seed":"0x1","counterexample":"[a()]"}"#,
        "\n",
        r#"{"type":"case","suite":"m","name":"q","outcome":"pass"}"#,
        "\n",
        r#"{"type":"run-end"}"#,
        "\n",
    ));
    let cases = &run.suites[0].cases;
    assert_eq!(cases[0].kind.as_deref(), Some("history"));
    assert_eq!(cases[0].seed.as_deref(), Some("0x1"));
    assert_eq!(cases[0].counterexample.as_deref(), Some("[a()]"));
    assert_eq!(cases[1].kind, None, "a case without them has none");
}

#[test]
fn parser_reads_the_violation_a_failed_case_names() {
    // #2272: a violation carries the declaration's kind, which the name alone cannot
    // say, and its `path:line:col`; a case without one has no `violation`.
    let run = parse_ndjson(concat!(
        r#"{"type":"suite-begin","name":"m","kind":"unit"}"#,
        "\n",
        r#"{"type":"case","suite":"m","name":"v","outcome":"fail","message":"Error: InvariantViolation: Meter.same","location":"unknown","violation":{"kind":"transition","agent":"Meter","name":"same","location":"src/demo/m.bynk:28:14"}}"#,
        "\n",
        r#"{"type":"case","suite":"m","name":"q","outcome":"pass"}"#,
        "\n",
        r#"{"type":"run-end"}"#,
        "\n",
    ));
    let cases = &run.suites[0].cases;
    let violation = cases[0].violation.as_ref().expect("the violation");
    assert_eq!(violation.kind, "transition");
    assert_eq!(violation.agent, "Meter");
    assert_eq!(violation.name, "same");
    assert_eq!(
        violation.location,
        Some(Location {
            path: "src/demo/m.bynk".to_string(),
            line: 28,
            col: 14,
        })
    );
    assert_eq!(
        cases[1].violation, None,
        "a passing case names no violation"
    );
}
