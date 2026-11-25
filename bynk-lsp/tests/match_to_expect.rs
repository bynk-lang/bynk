//! #4252: the `match` -> `expect … is` quick fix. The plan is pure over the
//! file text, so these tests drive `plan` directly: every offer is checked to
//! apply cleanly and stay canonical under `bynk-fmt`, and every refusal case
//! returns no edit.

use bynk_lsp::match_to_expect::plan;
use bynk_syntax::span::Span;

fn canon(src: &str) -> String {
    bynk_fmt::format_source(src, &bynk_fmt::FormatOptions::default()).expect("source formats")
}

fn apply(src: &str, needle: &str) -> Option<String> {
    let at = src.find(needle).unwrap_or_else(|| panic!("no `{needle}`"));
    let (span, new_text) = plan(src, Span::new(at, at))?;
    let mut out = src.to_string();
    out.replace_range(span.start..span.end, &new_text);
    Some(out)
}

fn suite(body: &str) -> String {
    canon(&format!("suite shop\n\ncase \"c\" {{\n{body}\n}}\n"))
}

#[test]
fn rewrites_a_wildcard_fallback_block_arms() {
    let src = suite(
        "let r = Order.parse(\"x\")\nmatch r {\nOk(v) => {\nexpect v.total == 10\nexpect v.id == 1\n}\n_ => {\nexpect false\n}\n}",
    );
    let out = apply(&src, "match r").expect("offered");
    assert_eq!(
        out,
        suite(
            "let r = Order.parse(\"x\")\nexpect r is Ok(v)\nexpect v.total == 10\nexpect v.id == 1"
        )
    );
    assert_eq!(canon(&out), out, "the edit is canonical");
}

#[test]
fn rewrites_expression_arms_and_a_remaining_variant() {
    let src = suite(
        "let r = Order.parse(\"x\")\nmatch r {\nErr(_) => expect false\nOk(v) => expect v.total == 10\n}",
    );
    let out = apply(&src, "match r").expect("offered");
    assert_eq!(
        out,
        suite("let r = Order.parse(\"x\")\nexpect r is Ok(v)\nexpect v.total == 10")
    );
}

#[test]
fn offers_on_a_nullary_pattern_without_bindings() {
    let src = suite(
        "let r = Order.parse(\"x\")\nmatch r {\nNone => { expect r.ok }\n_ => { expect false }\n}",
    );
    let out = apply(&src, "match r").expect("offered");
    assert!(out.contains("expect r is None"), "{out}");
}

#[test]
fn refuses_a_guarded_arm() {
    let src = suite(
        "let r = Order.parse(\"x\")\nmatch r {\nOk(v) if v.total > 1 => { expect v.id == 1 }\n_ => { expect false }\n}",
    );
    assert!(apply(&src, "match r").is_none());
}

#[test]
fn refuses_a_binding_nested_in_another_pattern() {
    let src = suite(
        "let r = Order.parse(\"x\")\nmatch r {\nOk(Some(v)) => { expect v.id == 1 }\n_ => { expect false }\n}",
    );
    assert!(apply(&src, "match r").is_none());
}

#[test]
fn refuses_a_match_that_is_a_value() {
    let src = suite(
        "let r = Order.parse(\"x\")\nlet n = match r {\nOk(v) => { v.id }\n_ => { expect false }\n}\nexpect n == 1",
    );
    assert!(apply(&src, "match r").is_none());
}

#[test]
fn refuses_other_arm_counts_and_other_bodies() {
    let three = suite(
        "let r = Order.parse(\"x\")\nmatch r {\nOk(v) => { expect v.id == 1 }\nErr(_) => { expect false }\n_ => { expect false }\n}",
    );
    assert!(apply(&three, "match r").is_none());
    let real_fallback = suite(
        "let r = Order.parse(\"x\")\nmatch r {\nOk(v) => { expect v.id == 1 }\n_ => { expect true }\n}",
    );
    assert!(apply(&real_fallback, "match r").is_none());
}

#[test]
fn refuses_a_name_already_declared_in_the_case() {
    let src = suite(
        "let v = 1\nlet r = Order.parse(\"x\")\nmatch r {\nOk(v) => { expect v.id == 1 }\n_ => { expect false }\n}",
    );
    assert!(apply(&src, "match r").is_none());
}

#[test]
fn refuses_when_a_comment_would_be_dropped() {
    // The formatter cannot hold a comment there, so the file is written raw.
    let with = "suite shop\n\ncase \"c\" {\n  let r = Order.parse(\"x\")\n  match r {\n    Ok(v) => { expect v.id == 1 }\n    // fail otherwise\n    _ => { expect false }\n  }\n}\n";
    assert!(apply(with, "match r").is_none());
    // The same source without the comment is offered, so the refusal above is
    // the comment's and not a parse failure's.
    let without = with.replace("    // fail otherwise\n", "");
    assert!(apply(&without, "match r").is_some());
}

#[test]
fn a_non_canonical_file_gets_the_hand_indented_rewrite() {
    // Four-space indent is not what `bynk-fmt` prints, so the edit is the raw
    // text, indented to the match.
    let src = "suite shop\n\ncase \"c\" {\n    let r = Order.parse(\"x\")\n    match r {\n        Ok(v) => {\n            expect v.id == 1\n            expect v.total == 2\n        }\n        _ => { expect false }\n    }\n}\n";
    assert_ne!(canon(src), src, "the fixture is deliberately non-canonical");
    let out = apply(src, "match r").expect("offered");
    assert_eq!(
        out,
        "suite shop\n\ncase \"c\" {\n    let r = Order.parse(\"x\")\n    expect r is Ok(v)\n    expect v.id == 1\n    expect v.total == 2\n}\n"
    );
}

#[test]
fn refuses_a_first_arm_fallback_whose_pattern_overlaps_the_kept_one() {
    // Open record, list and range patterns can match the same value as the
    // kept arm, so `expect p is Point { x, .. }` would pass where the original
    // failed; the same variant twice overlaps too.
    for (first, second) in [
        (
            "Point { x: 0, .. } => expect false",
            "Point { x, .. } => { expect x == 1 }",
        ),
        ("[0, ..] => expect false", "[a, ..] => { expect a == 1 }"),
        ("..0 => expect false", "0.. => { expect true }"),
        ("Some(_) => expect false", "Some(v) => { expect v == 1 }"),
    ] {
        let src = suite(&format!(
            "let p = Order.parse(\"x\")\nmatch p {{\n{first}\n{second}\n}}"
        ));
        assert!(apply(&src, "match p").is_none(), "{first} / {second}");
    }
}

#[test]
fn offers_a_first_arm_fallback_for_a_different_variant() {
    let src = suite(
        "let p = Order.parse(\"x\")\nmatch p {\nNone => expect false\nSome(v) => { expect v == 1 }\n}",
    );
    assert!(apply(&src, "match p").is_some());
}

#[test]
fn refuses_a_subject_that_is_not_a_plain_name_or_call() {
    let src = suite(
        "let a = 1\nlet b = 2\nmatch a + b {\nOk(v) => { expect v == 1 }\n_ => { expect false }\n}",
    );
    assert!(apply(&src, "match a + b").is_none());
}

#[test]
fn refuses_a_name_an_earlier_is_already_binds() {
    // A second rewrite in one case would collide with the first's `v`.
    let src = suite(
        "let r = Order.parse(\"x\")\nexpect r is Ok(v)\nmatch r {\nOk(v) => { expect v.id == 1 }\n_ => { expect false }\n}",
    );
    assert!(apply(&src, "match r").is_none());
}

#[test]
fn refuses_outside_a_match() {
    let src = suite("let r = Order.parse(\"x\")\nexpect r.ok");
    assert!(apply(&src, "expect r.ok").is_none());
}
