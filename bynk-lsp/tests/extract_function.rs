//! #2100 slice 2: extract-function end to end. A real project on disk goes
//! through `diagnose_project` for its requirements, locals and expression
//! types, then the pure plan runs on it. A positive case re-diagnoses clean
//! (what `codeAction/resolve`'s DECISION B gate checks). Extracted functions
//! stay capability-free, and an early return (`?`) is never lifted; both
//! refusals carry a reason the server can show.

use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::{extract, fix_all};
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

struct Run {
    root: PathBuf,
    result: bynk_ide::ProjectDiagnostics,
}

impl Run {
    fn new(test_name: &str, src: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bynk-lsp-extract-fn-{}-{}",
            test_name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let p = root.join("app/m.bynk");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, src).unwrap();
        let root = root.canonicalize().unwrap_or(root);
        let sources =
            bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
        let result = bynk_ide::diagnose_project(&root, &sources);
        Run { root, result }
    }

    fn text(&self) -> &str {
        &self
            .result
            .files
            .iter()
            .find(|f| f.source_path == Path::new("app/m.bynk"))
            .expect("file analysed")
            .text
    }

    fn plan(&self, selected: &str) -> Result<extract::FunctionPlan, extract::FunctionRefusal> {
        let text = self.text();
        let start = text.find(selected).expect("the selection is in the text");
        let rel = Path::new("app/m.bynk");
        let none = Vec::new();
        let no_locals = Vec::new();
        let no_types = Vec::new();
        extract::plan_function(
            text,
            Span::new(start, start + selected.len()),
            self.result.requirements.get(rel).unwrap_or(&none),
            self.result.locals.get(rel).unwrap_or(&no_locals),
            self.result.expr_types.get(rel).unwrap_or(&no_types),
            &self.result.ty_intern,
        )
    }

    fn assert_clean(&self, extracted: &str) {
        let mut sources = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            self.root.clone(),
        ));
        sources.insert(self.root.join("app/m.bynk"), extracted.to_string());
        let post = bynk_ide::diagnose_project(&self.root, &sources);
        let residual: Vec<&str> = post
            .files
            .iter()
            .flat_map(|f| f.diagnostics.iter().map(|d| d.error.category))
            .collect();
        assert!(
            residual.is_empty(),
            "extracted:\n{extracted}\nresidual: {residual:?}"
        );
    }
}

const SERVICE: &str = "context app.m\n\ncapability Clock {\n  fn now() -> Effect[Int]\n}\n\nprovides Clock = Sys {\n  fn now() -> Effect[Int] {\n    1\n  }\n}\n\nservice s {\n  on call(x: Int) -> Effect[Int] given Clock {\n    let n <- Clock.now()\n    let y = x * 2 + 1\n    n + y\n  }\n}\n";

const RESULTS: &str = "commons app.m\n\nfn half(n: Int) -> Result[Int, String] {\n  if n < 0 {\n    Err(\"negative\")\n  } else {\n    Ok(n / 2)\n  }\n}\n\nfn quarter(n: Int) -> Result[Int, String] {\n  let h = half(n)?\n  Ok(h / 2)\n}\n";

#[test]
fn a_capability_free_selection_is_planned_and_rechecks_clean() {
    let run = Run::new("free", SERVICE);
    let plan = run.plan("x * 2 + 1").expect("a plan");
    assert_eq!(plan.name, "extractedFn");
    let extracted = fix_all::apply(run.text(), &{
        let mut e = plan.edits.clone();
        e.sort_by_key(|(s, _)| s.start);
        e
    });
    assert!(
        extracted.contains("fn extractedFn(x: Int) -> Int {"),
        "{extracted}"
    );
    assert!(extracted.contains("let y = extractedFn(x)"), "{extracted}");
    run.assert_clean(&extracted);
}

#[test]
fn a_capability_using_selection_is_refused_with_a_reason() {
    let run = Run::new("capability", SERVICE);
    let refusal = run.plan("Clock.now()").err().expect("refused");
    let reason = refusal.reason.expect("a reason the user sees");
    assert!(reason.contains("`Clock`"), "{reason}");
    assert!(reason.contains("capability-free"), "{reason}");
    assert!(reason.contains("`given`"), "{reason}");
}

#[test]
fn an_early_return_is_refused_with_a_reason() {
    let run = Run::new("question", RESULTS);
    let refusal = run.plan("half(n)?").err().expect("refused");
    let reason = refusal.reason.expect("a reason the user sees");
    assert!(reason.contains("`?`"), "{reason}");
    // Without the `?`, the call extracts.
    assert!(run.plan("half(n)").is_ok());
}

#[test]
fn nothing_to_extract_is_a_silent_refusal() {
    let run = Run::new("silent", SERVICE);
    let text = run.text();
    let rel = Path::new("app/m.bynk");
    // A bare cursor.
    let at = text.find("x * 2").unwrap();
    let refusal = extract::plan_function(
        text,
        Span::new(at, at),
        run.result.requirements.get(rel).unwrap(),
        run.result.locals.get(rel).unwrap(),
        run.result.expr_types.get(rel).unwrap(),
        &run.result.ty_intern,
    )
    .err()
    .expect("refused");
    assert!(refusal.reason.is_none());
}

#[test]
fn the_listing_is_lazy_or_disabled_with_its_reason() {
    let run = Run::new("listing", SERVICE);
    let uri = Url::from_file_path(run.root.join("app/m.bynk")).unwrap();
    let text = run.text();

    let span = |sel: &str| {
        let s = text.find(sel).unwrap();
        Span::new(s, s + sel.len())
    };
    let ok = run.plan("x * 2 + 1");
    let Some(CodeActionOrCommand::CodeAction(listed)) =
        extract::function_listed(&ok, span("x * 2 + 1"), &uri, Some(4), true)
    else {
        panic!("a listed action");
    };
    assert_eq!(listed.kind, Some(CodeActionKind::REFACTOR_EXTRACT));
    assert!(listed.edit.is_none() && listed.disabled.is_none());
    let target = extract::FunctionTarget::of(&listed).unwrap();
    assert_eq!(
        (target.requested.start, target.requested.end, target.version),
        (span("x * 2 + 1").start, span("x * 2 + 1").end, Some(4))
    );

    let refused = run.plan("Clock.now()");
    let Some(CodeActionOrCommand::CodeAction(disabled)) =
        extract::function_listed(&refused, span("Clock.now()"), &uri, Some(4), true)
    else {
        panic!("a disabled action");
    };
    assert!(disabled.edit.is_none());
    assert!(
        disabled
            .disabled
            .unwrap()
            .reason
            .contains("capability-free")
    );
    // A client that doesn't show disabled actions gets nothing.
    assert!(
        extract::function_listed(&refused, span("Clock.now()"), &uri, Some(4), false).is_none()
    );
}

#[test]
fn a_called_local_closure_is_threaded_as_a_parameter() {
    // `g` is a local closure the selection calls: lifted, the new function
    // takes it as a parameter rather than naming an unbound `g`.
    let src = "commons app.m\n\nfn f(a: Int) -> Int {\n  let g = (n: Int) => n + a\n  let r = g(2) * 3\n  r\n}\n";
    let run = Run::new("callee", src);
    let plan = run.plan("g(2) * 3").expect("a plan");
    let extracted = fix_all::apply(run.text(), &{
        let mut e = plan.edits.clone();
        e.sort_by_key(|(s, _)| s.start);
        e
    });
    assert!(extracted.contains("fn extractedFn(g: "), "{extracted}");
    assert!(extracted.contains("let r = extractedFn(g)"), "{extracted}");
    run.assert_clean(&extracted);
}
