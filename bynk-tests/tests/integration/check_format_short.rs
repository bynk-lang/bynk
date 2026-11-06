//! v0.38 (ADR 0071): `bynkc check --format short` emits one terse line per
//! diagnostic — `path:line:col: severity[category]: message` — the exact shape
//! the VS Code `bynkc` problem-matcher parses. This test pins that shape so the
//! matcher can't silently break.

/// A source with a deterministic type error (return type `Int`, body `String`).
const SRC: &str = "commons demo\n\nfn f() -> Int {\n  \"x\"\n}\n";

#[test]
fn short_format_lines_match_the_problem_matcher_shape() {
    let errors = bynkc::compile(SRC, "demo.bynk").expect_err("source has a type error");
    let out = bynkc::render_errors_short(&errors, SRC, "demo.bynk");

    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        errors.len(),
        "one line per diagnostic, newline-terminated"
    );

    for line in &lines {
        // path:line:col: severity[category]: message
        let rest = line
            .strip_prefix("demo.bynk:")
            .unwrap_or_else(|| panic!("no path prefix: {line:?}"));
        let (line_no, after) = rest.split_once(':').expect("line:col");
        let (col_no, after) = after.split_once(": ").expect("col: ");
        assert!(
            line_no.parse::<u32>().is_ok() && col_no.parse::<u32>().is_ok(),
            "1-indexed line/col: {line:?}"
        );
        let severity = after.split('[').next().unwrap();
        assert!(
            severity == "error" || severity == "warning",
            "severity word: {line:?}"
        );
        assert!(
            after.contains("[bynk.") && after.contains("]: "),
            "category in brackets + message: {line:?}"
        );
    }
}

#[test]
fn short_format_positions_are_one_indexed() {
    let errors = bynkc::compile(SRC, "demo.bynk").expect_err("type error");
    let out = bynkc::render_errors_short(&errors, SRC, "demo.bynk");
    // The bad expression `"x"` is on line 4. The first diagnostic points at it.
    let first = out.lines().next().unwrap();
    assert!(
        first.starts_with("demo.bynk:4:"),
        "expected the line-4 error first, got {first:?}"
    );
}
