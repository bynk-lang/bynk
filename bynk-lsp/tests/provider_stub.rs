//! #2100 slice 4: generate a provider stub end to end. A real project on disk
//! goes through `diagnose_project` for its diagnostics and binding index, then
//! the pure stub plan runs on it. Each positive case re-diagnoses clean (what
//! `codeAction/resolve`'s DECISION B gate checks) and round-trips through
//! `bynk-fmt`.

use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::{fix_all, provider_stub};
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

fn canon(src: &str) -> String {
    bynk_fmt::format_source(src, &bynk_fmt::FormatOptions::default()).expect("source formats")
}

struct Run {
    root: PathBuf,
    result: bynk_ide::ProjectDiagnostics,
}

impl Run {
    fn new(test_name: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bynk-lsp-provider-stub-{}-{}",
            test_name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for (rel, src) in files {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, src).unwrap();
        }
        let root = root.canonicalize().unwrap_or(root);
        let sources =
            bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
        let result = bynk_ide::diagnose_project(&root, &sources);
        Run { root, result }
    }

    fn file(&self, rel: &str) -> &bynk_ide::FileDiagnostics {
        self.result
            .files
            .iter()
            .find(|f| f.source_path == Path::new(rel))
            .expect("file analysed")
    }

    fn plan(&self, rel: &str, capability: &str) -> Option<provider_stub::Plan> {
        provider_stub::plan(&self.file(rel).text, capability, &self.result.index)
    }

    /// Apply `plan` to `rel`, assert the project is clean and the file
    /// canonical, return the stubbed text.
    fn stub(&self, rel: &str, plan: &provider_stub::Plan) -> String {
        let stubbed = fix_all::apply(&self.file(rel).text, &plan.edits);
        let mut sources = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            self.root.clone(),
        ));
        sources.insert(self.root.join(rel), stubbed.clone());
        let post = bynk_ide::diagnose_project(&self.root, &sources);
        let residual: Vec<&str> = post
            .files
            .iter()
            .flat_map(|f| f.diagnostics.iter().map(|d| d.error.category))
            .collect();
        assert!(
            residual.is_empty(),
            "stubbed:\n{stubbed}\nresidual: {residual:?}"
        );
        assert_eq!(
            canon(&stubbed),
            stubbed,
            "stubbed text is not fmt-canonical"
        );
        stubbed
    }
}

const CLOCK: &str = "capability Clock {\n  fn now() -> Effect[Int]\n  fn zone(name: String) -> Effect[Option[String]]\n  fn tick() -> Effect[()]\n}\n";
const SERVICE: &str = "service s {\n  on call() -> Effect[Int] given Clock {\n    let n <- Clock.now()\n    n\n  }\n}\n";

fn context(items: &str) -> String {
    canon(&format!("context app.c\n\n{items}"))
}

#[test]
fn a_required_capability_gets_a_stub_from_its_declaration() {
    let src = context(&format!("{CLOCK}\n{SERVICE}"));
    let run = Run::new("required", &[("app/c.bynk", src.as_str())]);
    let file = run.file("app/c.bynk");
    let at = file.text.find("Clock").unwrap();
    let requests = provider_stub::requested(&file.text, &file.diagnostics, Span::new(at, at));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].capability, "Clock");
    let plan = run.plan("app/c.bynk", "Clock").expect("a plan");
    assert_eq!(plan.provider, "ClockImpl");
    let stubbed = run.stub("app/c.bynk", &plan);
    assert_eq!(
        stubbed,
        context(&format!(
            "{CLOCK}\nprovides Clock = ClockImpl {{\n  fn now() -> Effect[Int] {{\n    0\n  }}\n\n  fn zone(name: String) -> Effect[Option[String]] {{\n    None\n  }}\n\n  fn tick() -> Effect[()] {{\n    ()\n  }}\n}}\n\n{SERVICE}"
        ))
    );
}

#[test]
fn an_exported_capability_gets_a_stub() {
    let src = context(&format!("exports capability {{ Clock }}\n\n{CLOCK}"));
    let run = Run::new("exported", &[("app/c.bynk", src.as_str())]);
    let file = run.file("app/c.bynk");
    let requests =
        provider_stub::requested(&file.text, &file.diagnostics, Span::new(0, file.text.len()));
    assert!(
        requests
            .iter()
            .any(|r| r.quick_fix && r.capability == "Clock")
    );
    let plan = run.plan("app/c.bynk", "Clock").expect("a plan");
    run.stub("app/c.bynk", &plan);
}

#[test]
fn the_cursor_on_the_declaration_offers_a_refactor() {
    let src = context(CLOCK);
    let run = Run::new("refactor", &[("app/c.bynk", src.as_str())]);
    let file = run.file("app/c.bynk");
    let at = file.text.find("Clock").unwrap();
    let requests = provider_stub::requested(&file.text, &file.diagnostics, Span::new(at, at));
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].quick_fix);
    let plan = run.plan("app/c.bynk", "Clock").expect("a plan");
    run.stub("app/c.bynk", &plan);
}

#[test]
fn a_provided_capability_gets_none_even_from_a_sibling_file() {
    let main = context(&format!("{CLOCK}\n{SERVICE}"));
    let other = context(
        "provides Clock = Sys {\n  fn now() -> Effect[Int] {\n    1\n  }\n\n  fn zone(name: String) -> Effect[Option[String]] {\n    None\n  }\n\n  fn tick() -> Effect[()] {\n    ()\n  }\n}\n",
    );
    let run = Run::new(
        "provided",
        &[
            ("app/c/main.bynk", main.as_str()),
            ("app/c/sys.bynk", other.as_str()),
        ],
    );
    assert!(run.plan("app/c/main.bynk", "Clock").is_none());
}

#[test]
fn an_operation_without_a_default_return_gets_none() {
    let src = context(
        "type Reading = { at: Int }\n\ncapability Sensor {\n  fn read() -> Effect[Reading]\n}\n",
    );
    let run = Run::new("record", &[("app/c.bynk", src.as_str())]);
    assert!(run.plan("app/c.bynk", "Sensor").is_none());
}

#[test]
fn the_provider_name_avoids_a_taken_one() {
    let src = context(&format!("type ClockImpl = {{ n: Int }}\n\n{CLOCK}"));
    let run = Run::new("taken", &[("app/c.bynk", src.as_str())]);
    let plan = run.plan("app/c.bynk", "Clock").expect("a plan");
    assert_eq!(plan.provider, "ClockImpl2");
    run.stub("app/c.bynk", &plan);
}

#[test]
fn a_commons_offers_nothing() {
    let src = canon("commons app.c\n\nfn f() -> Int {\n  1\n}\n");
    let run = Run::new("commons", &[("app/c.bynk", src.as_str())]);
    let file = run.file("app/c.bynk");
    assert!(
        provider_stub::requested(&file.text, &file.diagnostics, Span::new(0, file.text.len()))
            .is_empty()
    );
}

#[test]
fn the_listed_action_has_no_edit() {
    let src = context(CLOCK);
    let run = Run::new("listed", &[("app/c.bynk", src.as_str())]);
    let plan = run.plan("app/c.bynk", "Clock").unwrap();
    let uri = Url::from_file_path(run.root.join("app/c.bynk")).unwrap();
    for (quick_fix, kind) in [
        (true, CodeActionKind::QUICKFIX),
        (false, CodeActionKind::REFACTOR),
    ] {
        let CodeActionOrCommand::CodeAction(action) =
            provider_stub::listed(&plan, quick_fix, &uri, Some(2))
        else {
            panic!("expected a CodeAction");
        };
        assert_eq!(action.title, "Generate provider `ClockImpl` for `Clock`");
        assert_eq!(action.kind, Some(kind));
        assert!(action.edit.is_none());
        let target = provider_stub::Target::of(&action).unwrap();
        assert_eq!(
            (target.capability.as_str(), target.version),
            ("Clock", Some(2))
        );
    }
}

#[test]
fn the_provider_name_avoids_one_taken_in_a_sibling_file() {
    // `ClockImpl` is declared in another file of the same context: the stub
    // must not collide with it, or resolve's re-check would refuse it.
    let main = context(CLOCK);
    let other = context("type ClockImpl = { n: Int }\n");
    let run = Run::new(
        "siblingname",
        &[
            ("app/c/main.bynk", main.as_str()),
            ("app/c/other.bynk", other.as_str()),
        ],
    );
    let plan = run.plan("app/c/main.bynk", "Clock").expect("a plan");
    assert_eq!(plan.provider, "ClockImpl2");
    run.stub("app/c/main.bynk", &plan);
}
