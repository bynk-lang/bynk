//! #2100 slice 1: inline variable end to end. A real project on disk goes
//! through `diagnose_project` for its local-binding index, then the pure
//! inline plan runs on it. Each positive case re-diagnoses clean (what
//! `codeAction/resolve`'s DECISION B gate checks) and round-trips through
//! `bynk-fmt`.

use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::{fix_all, inline};
use tower_lsp::lsp_types::*;

fn canon(src: &str) -> String {
    bynk_fmt::format_source(src, &bynk_fmt::FormatOptions::default()).expect("source formats")
}

fn setup(test_name: &str, src: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-inline-{}-{}",
        test_name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let p = root.join("app/m.bynk");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, src).unwrap();
    root.canonicalize().unwrap_or(root)
}

struct Run {
    root: PathBuf,
    result: bynk_ide::ProjectDiagnostics,
}

impl Run {
    fn new(test_name: &str, src: &str) -> Self {
        let root = setup(test_name, src);
        let sources =
            bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
        let result = bynk_ide::diagnose_project(&root, &sources);
        Run { root, result }
    }

    fn file(&self) -> &bynk_ide::FileDiagnostics {
        self.result
            .files
            .iter()
            .find(|f| f.source_path == Path::new("app/m.bynk"))
            .expect("file analysed")
    }

    /// The plan with the cursor at the `nth` occurrence of `needle`.
    fn plan(&self, needle: &str, nth: usize) -> Option<inline::Plan> {
        let text = &self.file().text;
        let offset = text
            .match_indices(needle)
            .nth(nth)
            .unwrap_or_else(|| panic!("no occurrence {nth} of `{needle}`"))
            .0;
        let locals = self
            .result
            .locals
            .get(Path::new("app/m.bynk"))
            .cloned()
            .unwrap_or_default();
        inline::plan(text, offset, &locals)
    }

    /// Apply `plan`, assert the result is clean and canonical, return it.
    fn inline(&self, plan: &inline::Plan) -> String {
        let text = &self.file().text;
        let inlined = fix_all::apply(text, &plan.edits);
        let mut sources = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            self.root.clone(),
        ));
        sources.insert(self.root.join("app/m.bynk"), inlined.clone());
        let post = bynk_ide::diagnose_project(&self.root, &sources);
        let residual: Vec<&str> = post
            .files
            .iter()
            .flat_map(|f| f.diagnostics.iter().map(|d| d.error.category))
            .collect();
        assert!(
            residual.is_empty(),
            "inlined:\n{inlined}\nresidual: {residual:?}"
        );
        assert_eq!(
            canon(&inlined),
            inlined,
            "inlined text is not fmt-canonical"
        );
        inlined
    }
}

fn module(body: &str) -> String {
    canon(&format!(
        "commons app.m\n\ntype Box = {{ size: Int }}\n\nfn twice(n: Int) -> Int {{\n  n * 2\n}}\n\n{body}"
    ))
}

#[test]
fn inlines_a_single_use_parenthesising_a_compound_initialiser() {
    let run = Run::new(
        "single",
        &module("fn f(a: Int) -> Int {\n  let y = a + 1\n  y * 3\n}\n"),
    );
    let plan = run.plan("y =", 0).expect("a plan");
    assert_eq!(plan.name, "y");
    assert_eq!(
        run.inline(&plan),
        module("fn f(a: Int) -> Int {\n  (a + 1) * 3\n}\n")
    );
}

#[test]
fn inlines_every_use_of_a_pure_initialiser() {
    let run = Run::new(
        "every",
        &module("fn f(a: Int) -> Int {\n  let d = twice(a)\n  d + d\n}\n"),
    );
    let plan = run.plan("d =", 0).expect("a plan");
    assert_eq!(
        run.inline(&plan),
        module("fn f(a: Int) -> Int {\n  twice(a) + twice(a)\n}\n")
    );
}

#[test]
fn the_cursor_on_a_use_offers_the_same_inline() {
    let run = Run::new(
        "fromuse",
        &module("fn f(a: Int) -> Int {\n  let d = twice(a)\n  d + 1\n}\n"),
    );
    let from_def = run.plan("d =", 0).expect("from the definition");
    let from_use = run.plan("d + 1", 0).expect("from a use");
    assert_eq!(from_def.edits, from_use.edits);
}

#[test]
fn a_shorthand_field_use_keeps_its_field_name() {
    let run = Run::new(
        "shorthand",
        &module("fn f(a: Int) -> Box {\n  let size = twice(a)\n  Box { size }\n}\n"),
    );
    let plan = run.plan("size =", 0).expect("a plan");
    assert_eq!(
        run.inline(&plan),
        module("fn f(a: Int) -> Box {\n  Box { size: twice(a) }\n}\n")
    );
}

#[test]
fn a_same_named_field_is_not_a_use() {
    let run = Run::new(
        "fieldname",
        &module("fn f(a: Int) -> Box {\n  let size = twice(a)\n  Box { size: size }\n}\n"),
    );
    let plan = run.plan("size =", 0).expect("a plan");
    assert_eq!(
        run.inline(&plan),
        module("fn f(a: Int) -> Box {\n  Box { size: twice(a) }\n}\n")
    );
}

#[test]
fn an_effectful_binding_is_not_inlined() {
    let src = canon(
        "context app.m\n\ncapability Clock {\n  fn now() -> Effect[Int]\n}\n\nprovides Clock = Sys {\n  fn now() -> Effect[Int] {\n    1\n  }\n}\n\nservice s {\n  on call() -> Effect[Int] given Clock {\n    let t <- Clock.now()\n    t + 1\n  }\n}\n",
    );
    let run = Run::new("effect", &src);
    assert!(run.plan("t <-", 0).is_none());
}

#[test]
fn a_capture_by_a_shadowing_binding_is_refused() {
    // Inside the lambda `a` is the lambda's parameter, not `f`'s: inlining
    // `y` there would silently read the wrong `a`.
    let run = Run::new(
        "capture",
        &module("fn f(a: Int) -> Int {\n  let y = a + 1\n  let g = (a: Int) => y + a\n  g(2)\n}\n"),
    );
    assert!(run.plan("y =", 0).is_none());
}

#[test]
fn an_unused_binding_is_not_inlined() {
    let run = Run::new(
        "unused",
        &module("fn f(a: Int) -> Int {\n  let y = a + 1\n  a\n}\n"),
    );
    assert!(run.plan("y =", 0).is_none());
}

#[test]
fn a_commented_binding_is_not_inlined() {
    let run = Run::new(
        "comment",
        &module("fn f(a: Int) -> Int {\n  -- the next one\n  let y = a + 1\n  y * 3\n}\n"),
    );
    assert!(run.plan("y =", 0).is_none());
}

#[test]
fn a_cursor_off_any_binding_offers_nothing() {
    let run = Run::new(
        "nothing",
        &module("fn f(a: Int) -> Int {\n  let y = a + 1\n  y * 3\n}\n"),
    );
    assert!(run.plan("twice", 0).is_none());
}

#[test]
fn the_listed_action_is_a_refactor_inline_without_an_edit() {
    let run = Run::new(
        "listed",
        &module("fn f(a: Int) -> Int {\n  let y = a + 1\n  y * 3\n}\n"),
    );
    let plan = run.plan("y =", 0).unwrap();
    let uri = Url::from_file_path(run.root.join("app/m.bynk")).unwrap();
    let CodeActionOrCommand::CodeAction(action) = inline::listed(&plan, &uri, Some(3)) else {
        panic!("expected a CodeAction");
    };
    assert_eq!(action.title, "Inline `y`");
    assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_INLINE));
    assert!(action.edit.is_none());
    let target = inline::Target::of(&action).unwrap();
    assert_eq!((target.def, target.version), (plan.def.start, Some(3)));
}
