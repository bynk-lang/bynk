//! v0.43 (ADR 0075): string-interpolation end-to-end behaviour — the
//! hole-typing rule and the template-literal emission, exercised through the
//! single-file `compile` pipeline (lex → parse → resolve → check → emit).

/// Wrap declarations in a minimal `commons` so `bynkc::compile` can run them.
fn commons_with(decls: &str) -> String {
    format!("commons demo\n\n{decls}\n")
}

#[test]
fn scalar_holes_emit_a_template_literal() {
    let src = commons_with(
        r#"fn describe(name: String, count: Int, ratio: Float, ready: Bool) -> String { "name=\(name) count=\(count) ratio=\(ratio) ready=\(ready)" }"#,
    );
    let ts = bynkc::compile(&src, "demo.bynk").expect("scalar holes type-check");
    assert!(
        ts.contains("`name=${String(name)} count=${String(count)} ratio=${String(ratio)} ready=${String(ready)}`"),
        "expected a template literal with String()-wrapped holes; got:\n{ts}"
    );
}

#[test]
fn a_refined_hole_widens_to_its_base() {
    // A refinement of `String` displays as its base — the headline use case
    // (`Subject` in the hello-world example).
    let src = commons_with(
        r#"type Name = String where NonEmpty

fn greet(who: Name) -> String { "Hi, \(who)!" }"#,
    );
    let ts = bynkc::compile(&src, "demo.bynk").expect("a refined hole type-checks");
    assert!(ts.contains("`Hi, ${String(who)}!`"), "got:\n{ts}");
}

#[test]
fn a_plain_string_stays_a_double_quoted_literal() {
    // No hole → no template literal; the `StrLit` fast-path is untouched.
    let src = commons_with(r#"fn hi() -> String { "plain" }"#);
    let ts = bynkc::compile(&src, "demo.bynk").unwrap();
    assert!(ts.contains("\"plain\""), "got:\n{ts}");
    assert!(!ts.contains("`plain`"));
}

#[test]
fn a_non_scalar_hole_is_rejected() {
    // An `Option` has no display form — it must be mapped to a String first.
    let src = commons_with(r#"fn show(maybe: Option[Int]) -> String { "value: \(maybe)" }"#);
    let errs = bynkc::compile(&src, "demo.bynk").expect_err("a non-scalar hole is an error");
    assert!(
        errs.iter()
            .any(|e| e.category == "bynk.types.interpolation_non_scalar"),
        "expected interpolation_non_scalar; got {errs:?}"
    );
}

#[test]
fn special_chars_in_chunks_are_escaped_for_the_template() {
    // Backtick and `$` in the literal text must be escaped so they cannot
    // start a template substitution or close the literal.
    let src = commons_with(r#"fn money(n: Int) -> String { "cost is $\(n) `each`" }"#);
    let ts = bynkc::compile(&src, "demo.bynk").unwrap();
    assert!(
        ts.contains("`cost is \\$${String(n)} \\`each\\``"),
        "expected `$` and backtick escaped; got:\n{ts}"
    );
}
