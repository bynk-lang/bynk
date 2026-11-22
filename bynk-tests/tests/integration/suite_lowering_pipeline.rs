//! 1B.2b (#4028): the lowering tests that drove the driver's in-memory pipeline over a suite
//! (a `?` in a test case, the expression form of `expect`), moved from `bynk-emit`
//! (`emitter/lower.rs`).

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::emit_project;

#[test]
fn an_expect_expression_is_a_node_with_the_same_bytes() {
    // `_ => expect <pred>` is the expression form (the statement form has
    // its own structural report). Quotes and non-ASCII in the predicate's
    // source text are escaped exactly as `escape_ts_string` does.
    let out = emit_project(
        &[
            ("a/m.bynk", "commons a.m\n\nfn one() -> Int { 1 }\n"),
            (
                "tests/m.bynk",
                "suite a.m\n\ncase \"c\" {\n  let r = match one() {\n    _ => expect \"q\\\"\u{e9}\" == \"x\"\n  }\n}\n",
            ),
        ],
        bynk_check::project_model::BuildTarget::Bundle,
    );
    let text: String = out.docs.values().cloned().collect();
    let at = text
        .find("__bynkExpect((")
        .unwrap_or_else(|| panic!("no expression-form expect in:\n{text}"));
    let line = text[at..].lines().next().unwrap_or_default();
    let want_src = bynk_emit::emitter::escape_ts_string("expect \"q\\\"\u{e9}\" == \"x\"");
    assert!(line.contains(&want_src), "{line}");
    assert!(line.starts_with("__bynkExpect((\""), "{line}");
}

/// The emitted suite module for a one-case suite over `src`.
fn suite_ts(src: &str, suite: &str) -> String {
    let out = emit_project(
        &[("demo/day.bynk", src), ("tests/day.bynk", suite)],
        bynk_check::project_model::BuildTarget::Bundle,
    );
    out.text("tests/demo_day.test.ts").to_string()
}

const DAY: &str = "commons demo.day\n\ntype Day = String where NonEmpty\n\nfn len(d: Day) -> Int { d.length() }\n";

#[test]
fn a_question_in_a_test_case_fails_the_case_from_nodes() {
    // #2857: the case form is an `InlineBlock` holding the `throw`; it
    // prints on the one line the text form has, escapes included.
    let ts = suite_ts(
        DAY,
        "suite demo.day\n\ncase \"q\" {\n  let d = Day.of(\"x\")?\n  expect len(d) == 1\n}\n",
    );
    let at = ts.find("if (__r").expect("a `?` guard");
    let line = ts[at..].lines().next().unwrap();
    assert!(
        line.starts_with(
            "if (__r0.tag === \"Err\") { throw __bynkExpectFailure(\"tests/day.bynk:4:11\", "
        ) && line.ends_with("\\n  the operand was Err: \" + __bynkShow(__r0.error)); }"),
        "{line}"
    );
}

#[test]
fn a_question_in_a_hoisted_if_branch_in_a_test_case_is_nodes() {
    let ts = suite_ts(
        DAY,
        "suite demo.day\n\ncase \"q\" {\n  let c = true\n  let n = if c { len(Day.of(\"x\")?) } else { 0 }\n  expect n == 1\n}\n",
    );
    // The nested statements keep the printer's one-level indent per `if`
    // inside the case body.
    assert!(ts.contains("if (c) {\n"), "{ts}");
    let at = ts.find("let __r").expect("a hoisted slot");
    let slot_block: Vec<&str> = ts[at..].lines().take(8).collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let base = indent(slot_block[1]);
    assert_eq!(slot_block[1].trim_start(), "if (c) {");
    assert_eq!(indent(slot_block[2]), base + 2, "{slot_block:?}");
    assert!(
        slot_block[3].trim_start().starts_with("if (__r0.tag"),
        "{slot_block:?}"
    );
    assert_eq!(indent(slot_block[3]), base + 2, "{slot_block:?}");
    assert_eq!(indent(slot_block[5]), base, "{slot_block:?}");
}
