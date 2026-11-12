//! #2808: the help notes on `return_mismatch` / `argument_mismatch` (suggest
//! `<Type>.of(…)`) and on `+` with a `String` operand (suggest interpolation).
//! A negative fixture's `expected_error.txt` matches `category message` only, so
//! the notes and the fix are pinned here, in the way `suggestions.rs` pins fixes.

use std::path::Path;

use bynk_syntax::error::{Applicability, CompileError, Suggestion};

fn errors(source: &str) -> Vec<CompileError> {
    match bynk_emit::compile(source, "hints.bynk") {
        Ok(_) => panic!("expected a compile failure for:\n{source}"),
        Err(errors) => errors,
    }
}

fn sole(errors: &[CompileError], category: &str) -> CompileError {
    let matching: Vec<_> = errors.iter().filter(|e| e.category == category).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one `{category}`; got: {:?}",
        errors.iter().map(|e| e.category).collect::<Vec<_>>()
    );
    matching[0].clone()
}

fn has_of_hint(e: &CompileError, name: &str) -> bool {
    e.notes
        .iter()
        .any(|n| n.contains(&format!("use `{name}.of(value)`")))
}

fn apply(text: &str, s: &Suggestion) -> String {
    let mut edits = s.edits.clone();
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::new();
    let mut last = 0;
    for (span, replacement) in &edits {
        out.push_str(&text[last..span.start]);
        out.push_str(replacement);
        last = span.end;
    }
    out.push_str(&text[last..]);
    out
}

// -- `.of` hint --

#[test]
fn return_of_base_where_refined_expected_suggests_of() {
    let e = errors(
        "commons demo {\n  type Age = Int where Positive\n\n  fn age(n: Int) -> Age {\n    n\n  }\n}\n",
    );
    let e = sole(&e, "bynk.types.return_mismatch");
    assert!(has_of_hint(&e, "Age"), "notes: {:?}", e.notes);
}

#[test]
fn argument_of_base_where_refined_expected_suggests_of() {
    let e = errors(
        "commons demo {\n  type Age = Int where Positive\n\n  fn show(a: Age) -> Int {\n    1\n  }\n\n  fn caller(n: Int) -> Int {\n    show(n)\n  }\n}\n",
    );
    let e = sole(&e, "bynk.types.argument_mismatch");
    assert!(has_of_hint(&e, "Age"), "notes: {:?}", e.notes);
}

#[test]
fn opaque_local_type_suggests_of() {
    let e = errors(
        "commons demo {\n  type Code = opaque String\n\n  fn code(s: String) -> Code {\n    s\n  }\n}\n",
    );
    let e = sole(&e, "bynk.types.return_mismatch");
    assert!(has_of_hint(&e, "Code"), "notes: {:?}", e.notes);
}

#[test]
fn base_mismatch_gets_no_of_hint() {
    // `Age` is over `Int`; a `String` is not its base.
    let e = errors(
        "commons demo {\n  type Age = Int where Positive\n\n  fn age(s: String) -> Age {\n    s\n  }\n}\n",
    );
    let e = sole(&e, "bynk.types.return_mismatch");
    assert!(e.notes.is_empty(), "notes: {:?}", e.notes);
}

#[test]
fn consumed_opaque_type_gets_no_of_hint() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hints/src");
    let overlay =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &overlay);
    let diags: Vec<_> = result
        .files
        .iter()
        .filter(|f| f.source_path.to_string_lossy().replace('\\', "/") == "shop.bynk")
        .flat_map(|f| f.diagnostics.iter())
        .collect();
    let mismatch = diags
        .iter()
        .find(|d| d.error.category == "bynk.types.return_mismatch")
        .unwrap_or_else(|| {
            panic!(
                "no return_mismatch; got {:?}",
                diags.iter().map(|d| d.error.category).collect::<Vec<_>>()
            )
        });
    assert!(
        !has_of_hint(&mismatch.error, "Amount"),
        "`Amount.of` is refused here; notes: {:?}",
        mismatch.error.notes
    );
}

// -- interpolation hint --

const NOTE: &str = "string interpolation";

fn concat(body: &str) -> String {
    format!(
        "commons demo {{\n  type Tag = {{ label: String }}\n\n  fn f(name: String, n: Int, p: Tag, d: Duration) -> String {{\n    {body}\n  }}\n}}\n"
    )
}

fn plus_error(body: &str) -> (String, CompileError) {
    let src = concat(body);
    let e = sole(&errors(&src), "bynk.types.type_mismatch");
    (src, e)
}

#[test]
fn string_plus_suggests_interpolation_with_a_fix() {
    let (src, e) = plus_error("\"hi \" + name");
    assert!(e.notes.iter().any(|n| n.contains(NOTE)), "{:?}", e.notes);
    assert_eq!(e.suggestions.len(), 1);
    let s = &e.suggestions[0];
    assert_eq!(s.applicability, Applicability::MachineApplicable);
    let fixed = apply(&src, s);
    assert!(fixed.contains("\"hi \\(name)\""), "{fixed}");
    bynk_emit::compile(&fixed, "hints.bynk").expect("the applied fix compiles");
}

#[test]
fn field_path_and_int_operands_are_fixed() {
    let (src, e) = plus_error("p.label + \": \" + name");
    // The innermost `+` fires alone; its fix is exact on its own operands.
    let s = &e.suggestions[0];
    assert!(apply(&src, s).contains("\"\\(p.label): \" + name"));
    let (src, e) = plus_error("\"n=\" + n");
    let fixed = apply(&src, &e.suggestions[0]);
    assert!(fixed.contains("\"n=\\(n)\""), "{fixed}");
    bynk_emit::compile(&fixed, "hints.bynk").expect("the applied fix compiles");
}

#[test]
fn unsafe_operands_get_the_note_without_a_fix() {
    for body in [
        "\"say \\\"hi\\\" \" + name",
        "\"tab\\t\" + name",
        "\"a\" + \"\\(name)\"",
        "\"hi \" + name.toUpper()",
        "\"n=\" + 5",
    ] {
        let (_, e) = plus_error(body);
        assert!(
            e.notes.iter().any(|n| n.contains(NOTE)),
            "{body}: {:?}",
            e.notes
        );
        assert!(
            e.suggestions.is_empty(),
            "{body} offered a fix: {:?}",
            e.suggestions
        );
    }
}

#[test]
fn int_plus_without_string_gets_no_interpolation_note() {
    let src = concat("n + true").replace("-> String", "-> Int");
    let e = sole(&errors(&src), "bynk.types.type_mismatch");
    assert!(e.notes.is_empty(), "{:?}", e.notes);
}

// -- `average` over a `Decimal` (#3421) --

#[test]
fn average_over_a_decimal_key_says_why_and_points_at_sum_then_div() {
    let e = errors(
        "commons demo {\n  type Line = { price: Decimal }\n\n  fn mean(lines: List[Line]) -> Option[Float] {\n    lines.average((l) => l.price)\n  }\n}\n",
    );
    let e = sole(&e, "bynk.query.sum_needs_numeric");
    let notes = e.notes.join(" ");
    assert!(notes.contains("scale"), "notes: {:?}", e.notes);
    assert!(
        notes.contains("`sum`") && notes.contains("`div("),
        "notes: {:?}",
        e.notes
    );
    // `Decimal.div` takes a `Decimal` divisor, so a count has to become one.
    assert!(notes.contains("Decimal.fromInt"), "notes: {:?}", e.notes);
}

#[test]
fn average_over_a_non_numeric_key_keeps_the_numeric_key_note() {
    let e = errors(
        "commons demo {\n  type Line = { name: String }\n\n  fn mean(lines: List[Line]) -> Option[Float] {\n    lines.average((l) => l.name)\n  }\n}\n",
    );
    let e = sole(&e, "bynk.query.sum_needs_numeric");
    let notes = e.notes.join(" ");
    assert!(notes.contains("numeric keys are"), "notes: {:?}", e.notes);
    assert!(
        !notes.contains("scale"),
        "no Decimal advice for a String: {:?}",
        e.notes
    );
}

#[test]
fn sum_over_a_non_numeric_key_lists_decimal_among_the_numeric_keys() {
    let e = errors(
        "commons demo {\n  type Line = { name: String }\n\n  fn total(lines: List[Line]) -> Int {\n    lines.sum((l) => l.name)\n  }\n}\n",
    );
    let e = sole(&e, "bynk.query.sum_needs_numeric");
    assert!(
        e.notes.join(" ").contains("`Decimal`"),
        "`sum` takes a `Decimal`, so the note names it: {:?}",
        e.notes
    );
}
