//! #2100 slice 3: fill match arms end to end. A real project on disk goes
//! through `diagnose_project`; the checker attaches an arm suggestion to each
//! missing witness, and the pure fill computation turns them into one edit.
//! Each positive case re-diagnoses clean (what `codeAction/resolve`'s
//! DECISION B gate checks) and round-trips through `bynk-fmt`.

use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::{code_actions, fill_arms, fix_all};
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

fn canon(src: &str) -> String {
    bynk_fmt::format_source(src, &bynk_fmt::FormatOptions::default()).expect("source formats")
}

const SHAPE: &str = "type Shape =\n  | Circle(r: Int)\n  | Rect(w: Int, h: Int)\n  | Dot\n\ntype Box = { size: Int }\n\n";

fn setup(test_name: &str, src: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-fill-arms-{}-{}",
        test_name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let p = root.join("app/m.bynk");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, src).unwrap();
    root.canonicalize().unwrap_or(root)
}

fn diagnose(root: &Path, text: Option<&str>) -> (String, Vec<bynk_ide::Diagnostic>) {
    let mut sources = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
        root.to_path_buf(),
    ));
    if let Some(text) = text {
        sources.insert(root.join("app/m.bynk"), text.to_string());
    }
    let result = bynk_ide::diagnose_project(root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("app/m.bynk"))
        .expect("file analysed");
    (file.text.clone(), file.diagnostics.clone())
}

/// `commons app.m` with the shape types and `body` (top-level items).
fn module(body: &str) -> String {
    canon(&format!("commons app.m\n\n{SHAPE}{body}"))
}

/// Fill every fillable match in `src` (one at a time, re-diagnosing between)
/// and return the result, asserting it is clean and canonical.
fn fill(test_name: &str, src: &str) -> String {
    let root = setup(test_name, src);
    let (text, diags) = diagnose(&root, None);
    let spans = fill_arms::fillable(&diags, Span::new(0, text.len()));
    assert_eq!(spans.len(), 1, "one fillable match: {:?}", cats(&diags));
    let edits = fill_arms::edits(&text, &diags, spans[0]).expect("fill edits");
    let filled = fix_all::apply(&text, &edits);
    let (_, post) = diagnose(&root, Some(&filled));
    assert!(
        post.is_empty(),
        "filled:\n{filled}\nresidual: {:?}",
        cats(&post)
    );
    assert_eq!(canon(&filled), filled, "filled text is not fmt-canonical");
    filled
}

fn cats(diags: &[bynk_ide::Diagnostic]) -> Vec<&'static str> {
    diags.iter().map(|d| d.error.category).collect()
}

fn not_fillable(test_name: &str, src: &str) {
    let root = setup(test_name, src);
    let (text, diags) = diagnose(&root, None);
    assert!(
        cats(&diags).contains(&"bynk.types.non_exhaustive_match"),
        "not a non-exhaustive match: {:?}",
        cats(&diags)
    );
    assert!(
        fill_arms::fillable(&diags, Span::new(0, text.len())).is_empty(),
        "unexpectedly fillable: {:?}",
        cats(&diags)
    );
}

#[test]
fn fills_one_missing_variant() {
    let filled = fill(
        "one",
        &module(
            "fn area(s: Shape) -> Int {\n  match s {\n    Circle(_) => 1,\n    Rect(_, _) => 2,\n  }\n}\n",
        ),
    );
    assert_eq!(
        filled,
        module(
            "fn area(s: Shape) -> Int {\n  match s {\n    Circle(_) => 1,\n    Rect(_, _) => 2,\n    Dot => 0,\n  }\n}\n"
        )
    );
}

#[test]
fn fills_several_in_declaration_order_with_full_payload_patterns() {
    let filled = fill(
        "several",
        &module("fn label(s: Shape) -> String {\n  match s {\n    Dot => \"dot\",\n  }\n}\n"),
    );
    assert_eq!(
        filled,
        module(
            "fn label(s: Shape) -> String {\n  match s {\n    Dot => \"dot\",\n    Circle(_) => \"\",\n    Rect(_, _) => \"\",\n  }\n}\n"
        )
    );
}

#[test]
fn fills_nested_witnesses() {
    let filled = fill(
        "nested",
        &module(
            "fn size(o: Option[Shape]) -> Int {\n  match o {\n    None => 0,\n    Some(Dot) => 1,\n  }\n}\n",
        ),
    );
    assert!(
        filled.contains("\t\tSome(Circle(_)) => 0,\n\t\tSome(Rect(_, _)) => 0,\n"),
        "{filled}"
    );
}

#[test]
fn fills_a_bool_match() {
    let filled = fill(
        "bool",
        &module("fn pick(b: Bool) -> Option[Int] {\n  match b {\n    true => Some(1),\n  }\n}\n"),
    );
    assert!(
        filled.contains("\t\ttrue => Some(1),\n\t\tfalse => None,\n"),
        "{filled}"
    );
}

#[test]
fn an_unbounded_literal_match_gets_a_wildcard() {
    let filled = fill(
        "literal",
        &module("fn names(n: Int) -> List[String] {\n  match n {\n    0 => [\"zero\"],\n  }\n}\n"),
    );
    assert!(
        filled.contains("\t\t0 => [\"zero\"],\n\t\t_ => [],\n"),
        "{filled}"
    );
}

#[test]
fn an_expected_effect_takes_a_plain_default() {
    let filled = fill(
        "effect",
        &module(
            "fn later(s: Shape) -> Effect[Int] {\n  match s {\n    Dot => 1,\n    Circle(_) => 2,\n  }\n}\n",
        ),
    );
    assert!(filled.contains("\t\tRect(_, _) => 0,\n"), "{filled}");
}

#[test]
fn a_last_arm_without_a_comma_gets_one() {
    // Not canonical: the comma is optional in the grammar.
    let src = format!(
        "commons app.m\n\n{SHAPE}fn area(s: Shape) -> Int {{\n  match s {{\n    Circle(_) => 1,\n    Rect(_, _) => 2\n  }}\n}}\n"
    );
    let root = setup("nocomma", &src);
    let (text, diags) = diagnose(&root, None);
    let span = fill_arms::fillable(&diags, Span::new(0, text.len()))[0];
    let filled = fix_all::apply(&text, &fill_arms::edits(&text, &diags, span).unwrap());
    assert!(
        filled.contains("    Rect(_, _) => 2,\n    Dot => 0,\n  }"),
        "{filled}"
    );
    assert!(diagnose(&root, Some(&filled)).1.is_empty());
}

#[test]
fn a_comment_after_the_last_arm_stays_on_its_line() {
    // Not canonical: `bynk-fmt` can't yet keep a comment inside a `match`.
    let src = format!(
        "commons app.m\n\n{SHAPE}fn area(s: Shape) -> Int {{\n  match s {{\n    Circle(_) => 1,\n    Rect(_, _) => 2,  -- wide\n  }}\n}}\n"
    );
    let root = setup("comment", &src);
    let (text, diags) = diagnose(&root, None);
    let span = fill_arms::fillable(&diags, Span::new(0, text.len()))[0];
    let filled = fix_all::apply(&text, &fill_arms::edits(&text, &diags, span).unwrap());
    assert!(
        filled.contains("    Rect(_, _) => 2,  -- wide\n    Dot => 0,\n  }"),
        "{filled}"
    );
    assert!(diagnose(&root, Some(&filled)).1.is_empty());
}

#[test]
fn a_block_bodied_last_arm_fills_after_its_brace() {
    let filled = fill(
        "block",
        &module(
            "fn area(s: Shape) -> Int {\n  match s {\n    Dot => 0,\n    Circle(r) => {\n      let d = r * 2\n      d\n    },\n  }\n}\n",
        ),
    );
    assert!(
        filled.contains("\t\t\td\n\t\t},\n\t\tRect(_, _) => 0,\n\t}"),
        "{filled}"
    );
}

#[test]
fn a_result_type_without_a_default_is_not_fillable() {
    not_fillable(
        "record",
        &module("fn boxed(s: Shape) -> Box {\n  match s {\n    Dot => Box { size: 0 },\n  }\n}\n"),
    );
    not_fillable(
        "sum",
        &module("fn same(s: Shape) -> Shape {\n  match s {\n    Dot => Dot,\n  }\n}\n"),
    );
}

#[test]
fn a_let_else_match_is_not_fillable() {
    not_fillable(
        "letelse",
        &module(
            "fn radius(s: Shape) -> Int {\n  let Circle(r) = s else match {\n    Dot => 0,\n  }\n  r\n}\n",
        ),
    );
}

#[test]
fn an_exhaustive_match_is_not_fillable() {
    let src = module(
        "fn area(s: Shape) -> Int {\n  match s {\n    Circle(_) => 1,\n    Rect(_, _) => 2,\n    Dot => 3,\n  }\n}\n",
    );
    let root = setup("exhaustive", &src);
    let (text, diags) = diagnose(&root, None);
    assert!(diags.is_empty(), "{:?}", cats(&diags));
    assert!(fill_arms::fillable(&diags, Span::new(0, text.len())).is_empty());
}

#[test]
fn the_arm_suggestions_are_neither_one_click_nor_fix_all() {
    let src = module("fn area(s: Shape) -> Int {\n  match s {\n    Dot => 3,\n  }\n}\n");
    let root = setup("notoneclick", &src);
    let (text, diags) = diagnose(&root, None);
    assert!(!diags.is_empty());
    assert!(diags.iter().all(|d| !d.error.suggestions.is_empty()));
    let uri = Url::from_file_path(root.join("app/m.bynk")).unwrap();
    assert!(
        code_actions::quick_fixes(&text, &diags, Span::new(0, text.len()), &uri, None).is_empty()
    );
    assert!(fix_all::edits(&text, &diags).is_empty());
}

#[test]
fn the_listed_action_is_a_quick_fix_without_an_edit() {
    let src = module("fn area(s: Shape) -> Int {\n  match s {\n    Dot => 3,\n  }\n}\n");
    let root = setup("listed", &src);
    let (text, diags) = diagnose(&root, None);
    let span = fill_arms::fillable(&diags, Span::new(0, text.len()))[0];
    let uri = Url::from_file_path(root.join("app/m.bynk")).unwrap();
    let CodeActionOrCommand::CodeAction(action) = fill_arms::listed(&diags, span, &uri, Some(5))
    else {
        panic!("expected a CodeAction");
    };
    assert_eq!(action.title, "Fill 2 missing match arms");
    assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
    assert!(action.edit.is_none());
    let target = fill_arms::Target::of(&action).expect("our data");
    assert_eq!(
        (target.match_span.start, target.match_span.end),
        (span.start, span.end)
    );
    assert_eq!(target.version, Some(5));
    assert_eq!(target.uri, uri);
}
